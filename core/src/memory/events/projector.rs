//! Memory Event Sourcing — Event Projector
//!
//! [`EventProjector`] folds a stream of [`super::MemoryEvent`]s into a
//! current-state [`crate::memory::context::MemoryFact`] projection.
//! Used both for rebuilding read-side state and for time-travel queries.

use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::context::{
    compute_parent_path, FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact,
    MemoryLayer, MemoryScope, MemoryTier, TemporalScope,
};
use crate::memory::events::{EventActor, MemoryEvent, MemoryEventEnvelope};
use crate::resilience::database::StateDatabase;

/// Folds a stream of memory events into a current-state `MemoryFact`.
///
/// The projector is the read-side of the event-sourcing architecture:
/// it replays events in sequence order to reconstruct the current state
/// of a fact. It can also replay up to a specific timestamp for
/// time-travel queries.
///
/// ## Pure fold
///
/// [`EventProjector::fold_events_to_fact`] is a **pure function** — no I/O,
/// no side effects. This makes it trivially testable and deterministic.
///
/// ## Projection from store
///
/// [`EventProjector::rebuild_fact`] and [`EventProjector::rebuild_fact_at`]
/// load events from the [`StateDatabase`] and then delegate to the pure fold.
pub struct EventProjector {
    db: Arc<StateDatabase>,
}

impl EventProjector {
    /// Create a new projector backed by the given event store.
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    /// Pure fold: replay a sequence of events into a `MemoryFact`.
    ///
    /// Returns `Ok(None)` if:
    /// - The event list is empty
    /// - The fact was permanently deleted (`FactDeleted`)
    ///
    /// Events must be ordered by sequence number (ascending).
    /// A `FactCreated` or `FactMigrated` event must appear before any
    /// mutation events; if a mutation arrives before initialization,
    /// it is silently skipped.
    pub fn fold_events_to_fact(
        events: &[MemoryEventEnvelope],
    ) -> Result<Option<MemoryFact>, AlephError> {
        if events.is_empty() {
            return Ok(None);
        }

        let mut fact: Option<MemoryFact> = None;

        for envelope in events {
            match &envelope.event {
                // --------------------------------------------------------
                // Initialization events
                // --------------------------------------------------------
                MemoryEvent::FactCreated {
                    fact_id,
                    content,
                    fact_type,
                    tier,
                    scope,
                    path,
                    namespace,
                    workspace,
                    confidence,
                    source,
                    source_memory_ids,
                } => {
                    let parent_path = compute_parent_path(path);
                    let category = fact_type.default_category();

                    fact = Some(MemoryFact {
                        id: fact_id.clone(),
                        content: content.clone(),
                        fact_type: fact_type.clone(),
                        embedding: None,
                        source_memory_ids: source_memory_ids.clone(),
                        created_at: envelope.timestamp,
                        updated_at: envelope.timestamp,
                        confidence: *confidence,
                        is_valid: true,
                        invalidation_reason: None,
                        decay_invalidated_at: None,
                        specificity: FactSpecificity::default(),
                        temporal_scope: TemporalScope::default(),
                        namespace: namespace.clone(),
                        workspace: workspace.clone(),
                        similarity_score: None,
                        path: path.clone(),
                        layer: MemoryLayer::default(),
                        category,
                        fact_source: *source,
                        content_hash: String::new(), // recomputed at projection time
                        parent_path,
                        embedding_model: String::new(), // set at projection time
                        tier: *tier,
                        scope: *scope,
                        persona_id: None,
                        strength: 1.0,
                        access_count: 0,
                        last_accessed_at: None,
                    });
                }

                MemoryEvent::FactMigrated { snapshot, .. } => {
                    let migrated: MemoryFact =
                        serde_json::from_value(snapshot.clone()).map_err(|e| AlephError::Other {
                            message: format!("Failed to deserialize FactMigrated snapshot: {e}"),
                            suggestion: None,
                        })?;
                    fact = Some(migrated);
                }

                // --------------------------------------------------------
                // Mutation events (require an initialized fact)
                // --------------------------------------------------------
                MemoryEvent::FactContentUpdated {
                    new_content, ..
                } => {
                    if let Some(ref mut f) = fact {
                        f.content = new_content.clone();
                        f.content_hash = String::new(); // recomputed at projection time
                        f.updated_at = envelope.timestamp;
                    }
                }

                MemoryEvent::FactMetadataUpdated {
                    field, new_value, ..
                } => {
                    if let Some(ref mut f) = fact {
                        match field.as_str() {
                            "tier" => {
                                f.tier = MemoryTier::from_str_or_default(new_value);
                            }
                            "scope" => {
                                f.scope = MemoryScope::from_str_or_default(new_value);
                            }
                            "path" => {
                                f.path = new_value.clone();
                                f.parent_path = compute_parent_path(new_value);
                            }
                            "namespace" => {
                                f.namespace = new_value.clone();
                            }
                            "workspace" => {
                                f.workspace = new_value.clone();
                            }
                            _ => {
                                // Unknown metadata field — silently ignore
                            }
                        }
                        f.updated_at = envelope.timestamp;
                    }
                }

                MemoryEvent::TierTransitioned { to_tier, .. } => {
                    if let Some(ref mut f) = fact {
                        f.tier = *to_tier;
                        f.updated_at = envelope.timestamp;
                    }
                }

                MemoryEvent::FactAccessed {
                    new_access_count, ..
                } => {
                    if let Some(ref mut f) = fact {
                        f.access_count = *new_access_count;
                        f.last_accessed_at = Some(envelope.timestamp);
                    }
                }

                MemoryEvent::StrengthDecayed {
                    new_strength, ..
                } => {
                    if let Some(ref mut f) = fact {
                        f.strength = *new_strength;
                    }
                }

                MemoryEvent::FactInvalidated {
                    reason, actor, ..
                } => {
                    if let Some(ref mut f) = fact {
                        f.is_valid = false;
                        f.invalidation_reason = Some(reason.clone());
                        if *actor == EventActor::Decay {
                            f.decay_invalidated_at = Some(envelope.timestamp);
                        }
                    }
                }

                MemoryEvent::FactRestored {
                    new_strength, ..
                } => {
                    if let Some(ref mut f) = fact {
                        f.is_valid = true;
                        f.invalidation_reason = None;
                        f.decay_invalidated_at = None;
                        f.strength = *new_strength;
                    }
                }

                MemoryEvent::FactDeleted { .. } => {
                    return Ok(None);
                }

                MemoryEvent::FactConsolidated {
                    consolidated_content,
                    ..
                } => {
                    if let Some(ref mut f) = fact {
                        f.content = consolidated_content.clone();
                        f.updated_at = envelope.timestamp;
                    }
                }
            }
        }

        Ok(fact)
    }

    /// Rebuild a fact by loading all events from the store and folding them.
    pub async fn rebuild_fact(
        &self,
        fact_id: &str,
    ) -> Result<Option<MemoryFact>, AlephError> {
        let events = self.db.get_memory_events_for_fact(fact_id).await?;
        Self::fold_events_to_fact(&events)
    }

    /// Rebuild a fact at a specific point in time.
    ///
    /// Only events with `timestamp <= at` are included in the fold.
    pub async fn rebuild_fact_at(
        &self,
        fact_id: &str,
        at: i64,
    ) -> Result<Option<MemoryFact>, AlephError> {
        let events = self.db.get_memory_events_until(fact_id, at).await?;
        Self::fold_events_to_fact(&events)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::events::*;

    /// Helper: create a `MemoryEventEnvelope` wrapping a `FactCreated` event.
    fn make_created_envelope(fact_id: &str, seq: u64, ts: i64) -> MemoryEventEnvelope {
        MemoryEventEnvelope {
            id: 0,
            fact_id: fact_id.to_string(),
            seq,
            event: MemoryEvent::FactCreated {
                fact_id: fact_id.to_string(),
                content: "User prefers Rust".to_string(),
                fact_type: FactType::Preference,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                path: "aleph://user/preferences/".to_string(),
                namespace: "owner".to_string(),
                workspace: "default".to_string(),
                confidence: 0.9,
                source: FactSource::Extracted,
                source_memory_ids: vec!["mem-001".to_string()],
            },
            actor: EventActor::Agent,
            timestamp: ts,
            correlation_id: None,
        }
    }

    /// Helper: wrap an event in an envelope.
    fn wrap(fact_id: &str, seq: u64, ts: i64, event: MemoryEvent) -> MemoryEventEnvelope {
        MemoryEventEnvelope {
            id: 0,
            fact_id: fact_id.to_string(),
            seq,
            event,
            actor: EventActor::System,
            timestamp: ts,
            correlation_id: None,
        }
    }

    /// Helper: wrap with a specific actor.
    fn wrap_with_actor(
        fact_id: &str,
        seq: u64,
        ts: i64,
        event: MemoryEvent,
        actor: EventActor,
    ) -> MemoryEventEnvelope {
        MemoryEventEnvelope {
            id: 0,
            fact_id: fact_id.to_string(),
            seq,
            event,
            actor,
            timestamp: ts,
            correlation_id: None,
        }
    }

    // --- fold: empty ---------------------------------------------------------

    #[test]
    fn test_fold_empty_events() {
        let result = EventProjector::fold_events_to_fact(&[]).unwrap();
        assert!(result.is_none());
    }

    // --- fold: single FactCreated --------------------------------------------

    #[test]
    fn test_fold_single_created() {
        let env = make_created_envelope("fact-001", 1, 1000);
        let fact = EventProjector::fold_events_to_fact(&[env])
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.id, "fact-001");
        assert_eq!(fact.content, "User prefers Rust");
        assert_eq!(fact.fact_type, FactType::Preference);
        assert_eq!(fact.tier, MemoryTier::ShortTerm);
        assert_eq!(fact.scope, MemoryScope::Global);
        assert_eq!(fact.path, "aleph://user/preferences/");
        assert_eq!(fact.namespace, "owner");
        assert_eq!(fact.workspace, "default");
        assert!((fact.confidence - 0.9).abs() < f32::EPSILON);
        assert_eq!(fact.fact_source, FactSource::Extracted);
        assert_eq!(fact.source_memory_ids, vec!["mem-001"]);
        assert_eq!(fact.created_at, 1000);
        assert_eq!(fact.updated_at, 1000);
        assert!(fact.is_valid);
        assert!(fact.invalidation_reason.is_none());
        assert!(fact.decay_invalidated_at.is_none());
        assert!(fact.embedding.is_none());
        assert!(fact.similarity_score.is_none());
        assert!(fact.persona_id.is_none());
        assert!((fact.strength - 1.0).abs() < f32::EPSILON);
        assert_eq!(fact.access_count, 0);
        assert!(fact.last_accessed_at.is_none());
        assert_eq!(fact.parent_path, "aleph://user/");
        assert_eq!(fact.category, MemoryCategory::Preferences);
        assert_eq!(fact.layer, MemoryLayer::L2Detail);
        assert_eq!(fact.specificity, FactSpecificity::default());
        assert_eq!(fact.temporal_scope, TemporalScope::default());
    }

    // --- fold: FactCreated + FactContentUpdated ------------------------------

    #[test]
    fn test_fold_created_then_content_updated() {
        let events = vec![
            make_created_envelope("fact-002", 1, 1000),
            wrap(
                "fact-002",
                2,
                2000,
                MemoryEvent::FactContentUpdated {
                    fact_id: "fact-002".to_string(),
                    old_content: "User prefers Rust".to_string(),
                    new_content: "User prefers Rust and Go".to_string(),
                    reason: "correction".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.content, "User prefers Rust and Go");
        assert_eq!(fact.updated_at, 2000);
        assert_eq!(fact.created_at, 1000); // unchanged
    }

    // --- fold: FactCreated + FactInvalidated ---------------------------------

    #[test]
    fn test_fold_created_then_invalidated() {
        let events = vec![
            make_created_envelope("fact-003", 1, 1000),
            wrap_with_actor(
                "fact-003",
                2,
                3000,
                MemoryEvent::FactInvalidated {
                    fact_id: "fact-003".to_string(),
                    reason: "outdated information".to_string(),
                    actor: EventActor::User,
                    strength_at_invalidation: Some(0.5),
                },
                EventActor::User,
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce an invalidated fact");

        assert!(!fact.is_valid);
        assert_eq!(
            fact.invalidation_reason.as_deref(),
            Some("outdated information")
        );
        // User actor should NOT set decay_invalidated_at
        assert!(fact.decay_invalidated_at.is_none());
    }

    #[test]
    fn test_fold_created_then_invalidated_by_decay() {
        let events = vec![
            make_created_envelope("fact-003d", 1, 1000),
            wrap_with_actor(
                "fact-003d",
                2,
                3000,
                MemoryEvent::FactInvalidated {
                    fact_id: "fact-003d".to_string(),
                    reason: "strength below threshold".to_string(),
                    actor: EventActor::Decay,
                    strength_at_invalidation: Some(0.02),
                },
                EventActor::Decay,
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce an invalidated fact");

        assert!(!fact.is_valid);
        assert_eq!(fact.decay_invalidated_at, Some(3000));
    }

    // --- fold: FactCreated + FactDeleted → None ------------------------------

    #[test]
    fn test_fold_created_then_deleted() {
        let events = vec![
            make_created_envelope("fact-004", 1, 1000),
            wrap(
                "fact-004",
                2,
                4000,
                MemoryEvent::FactDeleted {
                    fact_id: "fact-004".to_string(),
                    reason: "user requested permanent removal".to_string(),
                },
            ),
        ];

        let result = EventProjector::fold_events_to_fact(&events).unwrap();
        assert!(result.is_none());
    }

    // --- fold: FactCreated + FactAccessed ------------------------------------

    #[test]
    fn test_fold_created_then_accessed() {
        let events = vec![
            make_created_envelope("fact-005", 1, 1000),
            wrap(
                "fact-005",
                2,
                5000,
                MemoryEvent::FactAccessed {
                    fact_id: "fact-005".to_string(),
                    query: Some("what language?".to_string()),
                    relevance_score: Some(0.95),
                    used_in_response: true,
                    new_access_count: 1,
                },
            ),
            wrap(
                "fact-005",
                3,
                6000,
                MemoryEvent::FactAccessed {
                    fact_id: "fact-005".to_string(),
                    query: None,
                    relevance_score: None,
                    used_in_response: false,
                    new_access_count: 2,
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.access_count, 2);
        assert_eq!(fact.last_accessed_at, Some(6000));
    }

    // --- fold: FactCreated + StrengthDecayed ---------------------------------

    #[test]
    fn test_fold_created_then_strength_decayed() {
        let events = vec![
            make_created_envelope("fact-006", 1, 1000),
            wrap(
                "fact-006",
                2,
                7000,
                MemoryEvent::StrengthDecayed {
                    fact_id: "fact-006".to_string(),
                    old_strength: 1.0,
                    new_strength: 0.85,
                    decay_factor: 0.95,
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        assert!((fact.strength - 0.85).abs() < f32::EPSILON);
    }

    // --- fold: FactCreated + Invalidated + Restored --------------------------

    #[test]
    fn test_fold_created_invalidated_restored() {
        let events = vec![
            make_created_envelope("fact-007", 1, 1000),
            wrap_with_actor(
                "fact-007",
                2,
                2000,
                MemoryEvent::FactInvalidated {
                    fact_id: "fact-007".to_string(),
                    reason: "decay below threshold".to_string(),
                    actor: EventActor::Decay,
                    strength_at_invalidation: Some(0.03),
                },
                EventActor::Decay,
            ),
            wrap(
                "fact-007",
                3,
                3000,
                MemoryEvent::FactRestored {
                    fact_id: "fact-007".to_string(),
                    new_strength: 0.6,
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a restored fact");

        assert!(fact.is_valid);
        assert!(fact.invalidation_reason.is_none());
        assert!(fact.decay_invalidated_at.is_none());
        assert!((fact.strength - 0.6).abs() < f32::EPSILON);
    }

    // --- fold: TierTransitioned ----------------------------------------------

    #[test]
    fn test_fold_tier_transitioned() {
        let events = vec![
            make_created_envelope("fact-008", 1, 1000),
            wrap(
                "fact-008",
                2,
                8000,
                MemoryEvent::TierTransitioned {
                    fact_id: "fact-008".to_string(),
                    from_tier: MemoryTier::ShortTerm,
                    to_tier: MemoryTier::LongTerm,
                    trigger: TierTransitionTrigger::Consolidation,
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.tier, MemoryTier::LongTerm);
        assert_eq!(fact.updated_at, 8000);
    }

    // --- fold: FactMigrated --------------------------------------------------

    #[test]
    fn test_fold_fact_migrated() {
        let snapshot = serde_json::json!({
            "id": "migrated-001",
            "content": "Migrated from legacy store",
            "fact_type": "learning",
            "embedding": null,
            "source_memory_ids": ["old-mem-1"],
            "created_at": 500,
            "updated_at": 900,
            "confidence": 0.75,
            "is_valid": true,
            "invalidation_reason": null,
            "decay_invalidated_at": null,
            "specificity": "pattern",
            "temporal_scope": "contextual",
            "namespace": "owner",
            "workspace": "default",
            "path": "aleph://knowledge/learning/",
            "layer": "l2_detail",
            "category": "entities",
            "fact_source": "extracted",
            "content_hash": "",
            "parent_path": "aleph://knowledge/",
            "embedding_model": "",
            "tier": "long_term",
            "scope": "global",
            "persona_id": null,
            "strength": 0.8,
            "access_count": 5,
            "last_accessed_at": 800
        });

        let events = vec![wrap(
            "migrated-001",
            1,
            10000,
            MemoryEvent::FactMigrated {
                fact_id: "migrated-001".to_string(),
                snapshot,
            },
        )];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a migrated fact");

        assert_eq!(fact.id, "migrated-001");
        assert_eq!(fact.content, "Migrated from legacy store");
        assert_eq!(fact.fact_type, FactType::Learning);
        assert_eq!(fact.tier, MemoryTier::LongTerm);
        assert!((fact.strength - 0.8).abs() < f32::EPSILON);
        assert_eq!(fact.access_count, 5);
        assert_eq!(fact.last_accessed_at, Some(800));
    }

    // --- fold: FactConsolidated ----------------------------------------------

    #[test]
    fn test_fold_consolidated() {
        let events = vec![
            make_created_envelope("fact-009", 1, 1000),
            wrap(
                "fact-009",
                2,
                9000,
                MemoryEvent::FactConsolidated {
                    fact_id: "fact-009".to_string(),
                    source_fact_ids: vec!["fact-a".to_string(), "fact-b".to_string()],
                    consolidated_content: "User prefers Rust, especially for systems programming"
                        .to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a consolidated fact");

        assert_eq!(
            fact.content,
            "User prefers Rust, especially for systems programming"
        );
        assert_eq!(fact.updated_at, 9000);
    }

    // --- fold: FactMetadataUpdated -------------------------------------------

    #[test]
    fn test_fold_metadata_updated_tier() {
        let events = vec![
            make_created_envelope("fact-010", 1, 1000),
            wrap(
                "fact-010",
                2,
                2000,
                MemoryEvent::FactMetadataUpdated {
                    fact_id: "fact-010".to_string(),
                    field: "tier".to_string(),
                    old_value: "short_term".to_string(),
                    new_value: "core".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.tier, MemoryTier::Core);
        assert_eq!(fact.updated_at, 2000);
    }

    #[test]
    fn test_fold_metadata_updated_path() {
        let events = vec![
            make_created_envelope("fact-011", 1, 1000),
            wrap(
                "fact-011",
                2,
                2000,
                MemoryEvent::FactMetadataUpdated {
                    fact_id: "fact-011".to_string(),
                    field: "path".to_string(),
                    old_value: "aleph://user/preferences/".to_string(),
                    new_value: "aleph://user/personal/identity/".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.path, "aleph://user/personal/identity/");
        assert_eq!(fact.parent_path, "aleph://user/personal/");
    }

    #[test]
    fn test_fold_metadata_updated_unknown_field_is_ignored() {
        let events = vec![
            make_created_envelope("fact-012", 1, 1000),
            wrap(
                "fact-012",
                2,
                2000,
                MemoryEvent::FactMetadataUpdated {
                    fact_id: "fact-012".to_string(),
                    field: "nonexistent_field".to_string(),
                    old_value: "a".to_string(),
                    new_value: "b".to_string(),
                },
            ),
        ];

        // Should not fail — unknown fields are silently skipped
        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        // updated_at is still bumped
        assert_eq!(fact.updated_at, 2000);
    }

    // --- fold: mutation before initialization is skipped ----------------------

    #[test]
    fn test_fold_mutation_before_created_is_skipped() {
        // A content-update without a preceding FactCreated should not panic
        let events = vec![wrap(
            "fact-orphan",
            1,
            1000,
            MemoryEvent::FactContentUpdated {
                fact_id: "fact-orphan".to_string(),
                old_content: String::new(),
                new_content: "orphan update".to_string(),
                reason: "test".to_string(),
            },
        )];

        let result = EventProjector::fold_events_to_fact(&events).unwrap();
        assert!(result.is_none());
    }

    // --- fold: complex multi-event sequence -----------------------------------

    #[test]
    fn test_fold_complex_sequence() {
        let events = vec![
            make_created_envelope("fact-complex", 1, 1000),
            wrap(
                "fact-complex",
                2,
                2000,
                MemoryEvent::FactAccessed {
                    fact_id: "fact-complex".to_string(),
                    query: Some("rust?".to_string()),
                    relevance_score: Some(0.9),
                    used_in_response: true,
                    new_access_count: 1,
                },
            ),
            wrap(
                "fact-complex",
                3,
                3000,
                MemoryEvent::StrengthDecayed {
                    fact_id: "fact-complex".to_string(),
                    old_strength: 1.0,
                    new_strength: 0.9,
                    decay_factor: 0.95,
                },
            ),
            wrap(
                "fact-complex",
                4,
                4000,
                MemoryEvent::TierTransitioned {
                    fact_id: "fact-complex".to_string(),
                    from_tier: MemoryTier::ShortTerm,
                    to_tier: MemoryTier::LongTerm,
                    trigger: TierTransitionTrigger::Reinforcement,
                },
            ),
            wrap(
                "fact-complex",
                5,
                5000,
                MemoryEvent::FactContentUpdated {
                    fact_id: "fact-complex".to_string(),
                    old_content: "User prefers Rust".to_string(),
                    new_content: "User strongly prefers Rust for systems programming".to_string(),
                    reason: "refined understanding".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.id, "fact-complex");
        assert_eq!(
            fact.content,
            "User strongly prefers Rust for systems programming"
        );
        assert_eq!(fact.tier, MemoryTier::LongTerm);
        assert!((fact.strength - 0.9).abs() < f32::EPSILON);
        assert_eq!(fact.access_count, 1);
        assert_eq!(fact.last_accessed_at, Some(2000));
        assert_eq!(fact.created_at, 1000);
        assert_eq!(fact.updated_at, 5000);
    }
}
