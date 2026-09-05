//! Memory Event Sourcing — Event Projector
//!
//! [`EventProjector`] folds a stream of [`super::MemoryEvent`]s into a
//! current-state [`crate::memory::context::MemoryFact`] projection.
//! Used both for rebuilding read-side state and for time-travel queries.

use crate::sync_primitives::Arc;

use crate::error::AlephError;
use crate::memory::context::{
    compute_parent_path, FactSpecificity, MemoryFact, MemoryLayer, TemporalScope,
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
/// [`EventProjector::fold_events_to_note`] is a **pure function** — no I/O,
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
    #[must_use]
    pub const fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    /// Pure fold: replay a sequence of events into a `MemoryFact`.
    ///
    /// Returns `Ok(None)` if:
    /// - The event list is empty
    /// - The fact was permanently deleted (`NoteDeleted`)
    ///
    /// Events must be ordered by sequence number (ascending).
    /// A `NoteCreated` or `NoteMigrated` event must appear before any
    /// mutation events; if a mutation arrives before initialization,
    /// it is silently skipped.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub fn fold_events_to_note(
        events: &[MemoryEventEnvelope],
    ) -> Result<Option<MemoryFact>, AlephError> {
        if events.is_empty() {
            return Ok(None);
        }

        let mut fact: Option<MemoryFact> = None;
        let mut access_count: u32 = 0;

        for envelope in events {
            match &envelope.event {
                // --------------------------------------------------------
                // Initialization events
                // --------------------------------------------------------
                MemoryEvent::NoteCreated {
                    note_path,
                    content,
                    note_type,
                    path,
                    namespace,
                    agent: workspace,
                    source,
                    source_memory_ids,
                } => {
                    let parent_path = compute_parent_path(path);
                    let category = note_type.default_category();

                    fact = Some(MemoryFact {
                        // rust-doctor-disable-next-line excessive-clone
                        id: note_path.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        content: content.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        note_type: note_type.clone(),
                        embedding: None,
                        // rust-doctor-disable-next-line excessive-clone
                        source_memory_ids: source_memory_ids.clone(),
                        created_at: envelope.timestamp,
                        updated_at: envelope.timestamp,
                        is_valid: true,
                        invalidation_reason: None,
                        decay_invalidated_at: None,
                        specificity: FactSpecificity::default(),
                        temporal_scope: TemporalScope::default(),
                        // rust-doctor-disable-next-line excessive-clone
                        namespace: namespace.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        agent: workspace.clone(),
                        similarity_score: None,
                        // rust-doctor-disable-next-line excessive-clone
                        path: path.clone(),
                        layer: MemoryLayer::default(),
                        category,
                        fact_source: *source,
                        // rust-doctor-disable-next-line unnecessary-allocation
                        content_hash: String::new(), // recomputed at projection time
                        parent_path,
                        // rust-doctor-disable-next-line unnecessary-allocation
                        embedding_model: String::new(), // set at projection time
                        persona_id: None,
                        access_count: 0,
                        last_accessed_at: None,
                        valid_from: None,
                        valid_to: None,
                    });
                }

                MemoryEvent::NoteMigrated { snapshot, .. } => {
                    // rust-doctor-disable-next-line excessive-clone
                    let migrated: MemoryFact =
                        // rust-doctor-disable-next-line excessive-clone
                        serde_json::from_value(snapshot.clone()).map_err(|e| {
                            AlephError::Other {
                                message: format!(
                                    "Failed to deserialize NoteMigrated snapshot: {e}"
                                ),
                                suggestion: None,
                            }
                        })?;
                    // Seed the access_count accumulator from the migrated
                    // snapshot; otherwise the final `f.access_count = access_count`
                    // assignment would reset it to 0.
                    access_count = migrated.access_count;
                    fact = Some(migrated);
                }

                // --------------------------------------------------------
                // Mutation events (require an initialized fact)
                // --------------------------------------------------------
                MemoryEvent::NoteContentUpdated { new_content, .. } => {
                    if let Some(ref mut f) = fact {
                        // rust-doctor-disable-next-line excessive-clone
                        f.content = new_content.clone();
                        // rust-doctor-disable-next-line unnecessary-allocation
                        f.content_hash = String::new(); // recomputed at projection time
                        f.updated_at = envelope.timestamp;
                    }
                }

                MemoryEvent::NoteMetadataUpdated {
                    field, new_value, ..
                } => {
                    if let Some(ref mut f) = fact {
                        match field.as_str() {
                            "path" => {
                                // rust-doctor-disable-next-line excessive-clone
                                f.path = new_value.clone();
                                f.parent_path = compute_parent_path(new_value);
                            }
                            "namespace" => {
                                // rust-doctor-disable-next-line excessive-clone
                                f.namespace = new_value.clone();
                            }
                            "agent" => {
                                // rust-doctor-disable-next-line excessive-clone
                                f.agent = new_value.clone();
                            }
                            _ => {
                                // Unknown metadata field — silently ignore
                            }
                        }
                        f.updated_at = envelope.timestamp;
                    }
                }

                MemoryEvent::NoteAccessed { .. } => {
                    if let Some(ref mut f) = fact {
                        access_count += 1;
                        f.last_accessed_at = Some(envelope.timestamp);
                    }
                }

                MemoryEvent::NoteInvalidated { reason, actor, .. } => {
                    if let Some(ref mut f) = fact {
                        f.is_valid = false;
                        // rust-doctor-disable-next-line excessive-clone
                        f.invalidation_reason = Some(reason.clone());
                        if *actor == EventActor::Decay {
                            f.decay_invalidated_at = Some(envelope.timestamp);
                        }
                    }
                }

                MemoryEvent::NoteRestored { .. } => {
                    if let Some(ref mut f) = fact {
                        f.is_valid = true;
                        f.invalidation_reason = None;
                        f.decay_invalidated_at = None;
                    }
                }

                MemoryEvent::NoteDeleted { .. } => {
                    return Ok(None);
                }

                MemoryEvent::NoteConsolidated {
                    consolidated_content,
                    ..
                } => {
                    if let Some(ref mut f) = fact {
                        // rust-doctor-disable-next-line excessive-clone
                        f.content = consolidated_content.clone();
                        f.updated_at = envelope.timestamp;
                    }
                }
            }
        }

        if let Some(ref mut f) = fact {
            f.access_count = access_count;
        }

        Ok(fact)
    }

    /// Rebuild a fact by loading all events from the store and folding them.
    pub async fn rebuild_fact(&self, fact_id: &str) -> Result<Option<MemoryFact>, AlephError> {
        let events = self.db.get_memory_events_for_fact(fact_id).await?;
        Self::fold_events_to_note(&events)
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
        Self::fold_events_to_note(&events)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::MemoryCategory;
    use crate::memory::events::*;

    /// Helper: create a `MemoryEventEnvelope` wrapping a `NoteCreated` event.
    fn make_created_envelope(fact_id: &str, seq: u64, ts: i64) -> MemoryEventEnvelope {
        MemoryEventEnvelope {
            id: 0,
            fact_id: fact_id.to_string(),
            seq,
            event: MemoryEvent::NoteCreated {
                note_path: fact_id.to_string(),
                content: "User prefers Rust".to_string(),
                note_type: NoteType::Preference,
                path: "aleph://user/preferences/".to_string(),
                namespace: "owner".to_string(),
                agent: "default".to_string(),
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
        let result = EventProjector::fold_events_to_note(&[]).unwrap();
        assert!(result.is_none());
    }

    // --- fold: single NoteCreated --------------------------------------------

    #[test]
    fn test_fold_single_created() {
        let env = make_created_envelope("fact-001", 1, 1000);
        let fact = EventProjector::fold_events_to_note(&[env])
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.id, "fact-001");
        assert_eq!(fact.content, "User prefers Rust");
        assert_eq!(fact.note_type, NoteType::Preference);
        assert_eq!(fact.path, "aleph://user/preferences/");
        assert_eq!(fact.namespace, "owner");
        assert_eq!(fact.agent, "default");
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
        assert_eq!(fact.access_count, 0);
        assert!(fact.last_accessed_at.is_none());
        assert_eq!(fact.parent_path, "aleph://user/");
        assert_eq!(fact.category, MemoryCategory::Preferences);
        assert_eq!(fact.layer, MemoryLayer::L2Detail);
        assert_eq!(fact.specificity, FactSpecificity::default());
        assert_eq!(fact.temporal_scope, TemporalScope::default());
    }

    // --- fold: NoteCreated + NoteContentUpdated ------------------------------

    #[test]
    fn test_fold_created_then_content_updated() {
        let events = vec![
            make_created_envelope("fact-002", 1, 1000),
            wrap(
                "fact-002",
                2,
                2000,
                MemoryEvent::NoteContentUpdated {
                    note_path: "fact-002".to_string(),
                    old_content: "User prefers Rust".to_string(),
                    new_content: "User prefers Rust and Go".to_string(),
                    reason: "correction".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.content, "User prefers Rust and Go");
        assert_eq!(fact.updated_at, 2000);
        assert_eq!(fact.created_at, 1000); // unchanged
    }

    // --- fold: NoteCreated + NoteInvalidated ---------------------------------

    #[test]
    fn test_fold_created_then_invalidated() {
        let events = vec![
            make_created_envelope("fact-003", 1, 1000),
            wrap_with_actor(
                "fact-003",
                2,
                3000,
                MemoryEvent::NoteInvalidated {
                    note_path: "fact-003".to_string(),
                    reason: "outdated information".to_string(),
                    actor: EventActor::User,
                },
                EventActor::User,
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
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
                MemoryEvent::NoteInvalidated {
                    note_path: "fact-003d".to_string(),
                    reason: "strength below threshold".to_string(),
                    actor: EventActor::Decay,
                },
                EventActor::Decay,
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce an invalidated fact");

        assert!(!fact.is_valid);
        assert_eq!(fact.decay_invalidated_at, Some(3000));
    }

    // --- fold: NoteCreated + NoteDeleted → None ------------------------------

    #[test]
    fn test_fold_created_then_deleted() {
        let events = vec![
            make_created_envelope("fact-004", 1, 1000),
            wrap(
                "fact-004",
                2,
                4000,
                MemoryEvent::NoteDeleted {
                    note_path: "fact-004".to_string(),
                    reason: "user requested permanent removal".to_string(),
                },
            ),
        ];

        let result = EventProjector::fold_events_to_note(&events).unwrap();
        assert!(result.is_none());
    }

    // --- fold: NoteCreated + NoteAccessed ------------------------------------

    #[test]
    fn test_fold_created_then_accessed() {
        let events = vec![
            make_created_envelope("fact-005", 1, 1000),
            wrap(
                "fact-005",
                2,
                5000,
                MemoryEvent::NoteAccessed {
                    note_path: "fact-005".to_string(),
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
                MemoryEvent::NoteAccessed {
                    note_path: "fact-005".to_string(),
                    query: None,
                    relevance_score: None,
                    used_in_response: false,
                    new_access_count: 2,
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.access_count, 2);
        assert_eq!(fact.last_accessed_at, Some(6000));
    }

    // --- fold: NoteCreated + Invalidated + Restored --------------------------

    #[test]
    fn test_fold_created_invalidated_restored() {
        let events = vec![
            make_created_envelope("fact-007", 1, 1000),
            wrap_with_actor(
                "fact-007",
                2,
                2000,
                MemoryEvent::NoteInvalidated {
                    note_path: "fact-007".to_string(),
                    reason: "decay below threshold".to_string(),
                    actor: EventActor::Decay,
                },
                EventActor::Decay,
            ),
            wrap(
                "fact-007",
                3,
                3000,
                MemoryEvent::NoteRestored {
                    note_path: "fact-007".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a restored fact");

        assert!(fact.is_valid);
        assert!(fact.invalidation_reason.is_none());
        assert!(fact.decay_invalidated_at.is_none());
    }

    // --- fold: NoteMigrated --------------------------------------------------

    #[test]
    fn test_fold_fact_migrated() {
        let snapshot = serde_json::json!({
            "id": "migrated-001",
            "content": "Migrated from legacy store",
            "note_type": "learning",
            "embedding": null,
            "source_memory_ids": ["old-mem-1"],
            "created_at": 500,
            "updated_at": 900,
            "is_valid": true,
            "invalidation_reason": null,
            "decay_invalidated_at": null,
            "specificity": "pattern",
            "temporal_scope": "contextual",
            "namespace": "owner",
            "agent": "default",
            "path": "aleph://knowledge/learning/",
            "layer": "l2_detail",
            "category": "entities",
            "fact_source": "extracted",
            "content_hash": "",
            "parent_path": "aleph://knowledge/",
            "embedding_model": "",
            "persona_id": null,
            "access_count": 5,
            "last_accessed_at": 800
        });

        let events = vec![wrap(
            "migrated-001",
            1,
            10000,
            MemoryEvent::NoteMigrated {
                note_path: "migrated-001".to_string(),
                snapshot,
            },
        )];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a migrated fact");

        assert_eq!(fact.id, "migrated-001");
        assert_eq!(fact.content, "Migrated from legacy store");
        assert_eq!(fact.note_type, NoteType::Learning);
        assert_eq!(fact.access_count, 5);
        assert_eq!(fact.last_accessed_at, Some(800));
    }

    // --- fold: NoteConsolidated ----------------------------------------------

    #[test]
    fn test_fold_consolidated() {
        let events = vec![
            make_created_envelope("fact-009", 1, 1000),
            wrap(
                "fact-009",
                2,
                9000,
                MemoryEvent::NoteConsolidated {
                    note_path: "fact-009".to_string(),
                    source_note_paths: vec!["fact-a".to_string(), "fact-b".to_string()],
                    consolidated_content: "User prefers Rust, especially for systems programming"
                        .to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a consolidated fact");

        assert_eq!(
            fact.content,
            "User prefers Rust, especially for systems programming"
        );
        assert_eq!(fact.updated_at, 9000);
    }

    // --- fold: NoteMetadataUpdated -------------------------------------------

    #[test]
    fn test_fold_metadata_updated_path_only() {
        let events = vec![
            make_created_envelope("fact-010", 1, 1000),
            wrap(
                "fact-010",
                2,
                2000,
                MemoryEvent::NoteMetadataUpdated {
                    note_path: "fact-010".to_string(),
                    field: "namespace".to_string(),
                    old_value: "owner".to_string(),
                    new_value: "guest".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.namespace, "guest");
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
                MemoryEvent::NoteMetadataUpdated {
                    note_path: "fact-011".to_string(),
                    field: "path".to_string(),
                    old_value: "aleph://user/preferences/".to_string(),
                    new_value: "aleph://user/personal/identity/".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
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
                MemoryEvent::NoteMetadataUpdated {
                    note_path: "fact-012".to_string(),
                    field: "nonexistent_field".to_string(),
                    old_value: "a".to_string(),
                    new_value: "b".to_string(),
                },
            ),
        ];

        // Should not fail — unknown fields are silently skipped
        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");

        // updated_at is still bumped
        assert_eq!(fact.updated_at, 2000);
    }

    // --- fold: mutation before initialization is skipped ----------------------

    #[test]
    fn test_fold_mutation_before_created_is_skipped() {
        // A content-update without a preceding NoteCreated should not panic
        let events = vec![wrap(
            "fact-orphan",
            1,
            1000,
            MemoryEvent::NoteContentUpdated {
                note_path: "fact-orphan".to_string(),
                old_content: String::new(),
                new_content: "orphan update".to_string(),
                reason: "test".to_string(),
            },
        )];

        let result = EventProjector::fold_events_to_note(&events).unwrap();
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
                MemoryEvent::NoteAccessed {
                    note_path: "fact-complex".to_string(),
                    query: Some("rust?".to_string()),
                    relevance_score: Some(0.9),
                    used_in_response: true,
                    new_access_count: 1,
                },
            ),
            wrap(
                "fact-complex",
                5,
                5000,
                MemoryEvent::NoteContentUpdated {
                    note_path: "fact-complex".to_string(),
                    old_content: "User prefers Rust".to_string(),
                    new_content: "User strongly prefers Rust for systems programming".to_string(),
                    reason: "refined understanding".to_string(),
                },
            ),
        ];

        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .expect("should produce a fact");

        assert_eq!(fact.id, "fact-complex");
        assert_eq!(
            fact.content,
            "User strongly prefers Rust for systems programming"
        );
        assert_eq!(fact.access_count, 1);
        assert_eq!(fact.last_accessed_at, Some(2000));
        assert_eq!(fact.created_at, 1000);
        assert_eq!(fact.updated_at, 5000);
    }
}
