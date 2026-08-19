//! Memory Event Sourcing — Memory Time Traveler
//!
//! [`MemoryTimeTraveler`] replays events up to a given point in time
//! to reconstruct the state of a fact (or the entire memory) as it was
//! at that moment. Useful for debugging, auditing, and undo operations.
//!
//! The traveler delegates to [`EventProjector::fold_events_to_note`] for
//! the actual state reconstruction, and provides higher-level query
//! methods for timeline and explanation use cases.

use crate::sync_primitives::Arc;

use crate::error::AlephError;
use crate::memory::events::{MemoryEvent, MemoryEventEnvelope};
use crate::memory::explain::{ExplainedEvent, FactExplanation};
use crate::resilience::database::StateDatabase;

use super::projector::EventProjector;

/// Time-travel service for historical memory state reconstruction.
///
/// Wraps the [`StateDatabase`] event store and provides:
/// - **`explain_fact`** — human-readable lifecycle explanation (replaces
///   `AuditLogger::explain_fact` for event-sourced facts)
pub struct MemoryTimeTraveler {
    db: Arc<StateDatabase>,
}

impl MemoryTimeTraveler {
    /// Create a new time traveler backed by the given event store.
    #[must_use]
    pub const fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    /// Explain a fact's lifecycle by converting its event stream into
    /// a [`FactExplanation`].
    ///
    /// This replaces `AuditLogger::explain_fact` for event-sourced facts
    /// and produces the same `FactExplanation` struct for backward
    /// compatibility. The explanation is derived entirely from the event
    /// stream — no separate audit log or fact table is required.
    pub async fn explain_fact(&self, fact_id: &str, agent_id: &str) -> Result<FactExplanation, AlephError> {
        let events = self.db.get_memory_events_for_fact(fact_id, agent_id).await?;
        if events.is_empty() {
            return Err(AlephError::other(format!(
                "No events found for fact {fact_id}"
            )));
        }

        // Convert events to ExplainedEvents and extract audit metadata
        let mut explained_events: Vec<ExplainedEvent> = Vec::with_capacity(events.len());
        let mut creation_source: Option<String> = None;
        let mut access_count: usize = 0;
        let mut invalidation_reason: Option<String> = None;

        for env in &events {
            explained_events.push(ExplainedEvent {
                timestamp: env.timestamp,
                action: env.event.event_type_tag().to_string(),
                description: describe_event(env),
                actor: env.actor.to_string(),
            });

            // Track audit metadata
            match &env.event {
                MemoryEvent::NoteCreated { source, .. } => {
                    creation_source = Some(format!("{source:?}"));
                }
                MemoryEvent::NoteMigrated { .. } => {
                    creation_source = Some("migration".to_string());
                }
                MemoryEvent::NoteAccessed { .. } => {
                    access_count += 1;
                }
                MemoryEvent::NoteInvalidated { reason, .. } => {
                    invalidation_reason = Some(reason.clone());
                }
                MemoryEvent::NoteRestored { .. } => {
                    // Restoration clears invalidation
                    invalidation_reason = None;
                }
                _ => {}
            }
        }

        // Reconstruct current state via projector
        let current_fact = EventProjector::fold_events_to_note(&events)?;

        let (content, is_valid) = match &current_fact {
            Some(f) => (Some(f.content.clone()), f.is_valid),
            None => (None, false), // deleted
        };

        Ok(FactExplanation {
            fact_id: fact_id.to_string(),
            content,
            is_valid,
            creation_source,
            access_count,
            invalidation_reason,
            events: explained_events,
        })
    }
}

/// Generate a human-readable description for an event envelope.
fn describe_event(env: &MemoryEventEnvelope) -> String {
    match &env.event {
        MemoryEvent::NoteCreated { content, .. } => {
            format!(
                "Fact created: \"{}\"",
                crate::utils::text_format::truncate_text(content, 50)
            )
        }
        MemoryEvent::NoteContentUpdated { reason, .. } => {
            format!("Content updated: {reason}")
        }
        MemoryEvent::NoteMetadataUpdated {
            field,
            old_value,
            new_value,
            ..
        } => {
            format!("Metadata '{field}' changed from '{old_value}' to '{new_value}'")
        }
        MemoryEvent::NoteAccessed {
            query,
            used_in_response,
            new_access_count,
            ..
        } => {
            format!(
                "Accessed (count: {new_access_count}, used: {used_in_response}, query: {query:?})"
            )
        }
        MemoryEvent::NoteInvalidated { reason, actor, .. } => {
            format!("Invalidated by {actor}: {reason}")
        }
        MemoryEvent::NoteRestored { .. } => "Restored".to_string(),
        MemoryEvent::NoteDeleted { reason, .. } => {
            format!("Permanently deleted: {reason}")
        }
        MemoryEvent::NoteConsolidated {
            source_note_paths, ..
        } => {
            format!("Consolidated from {} source notes", source_note_paths.len())
        }
        MemoryEvent::NoteMigrated { .. } => "Migrated from legacy CRUD store".to_string(),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, NoteType};
    use crate::memory::events::*;

    /// Helper: create an in-memory StateDatabase wrapped in Arc.
    fn make_db() -> Arc<StateDatabase> {
        Arc::new(StateDatabase::in_memory().unwrap())
    }

    /// Helper: build a NoteCreated envelope with a given timestamp.
    fn make_created(fact_id: &str, seq: u64, ts: i64) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::NoteCreated {
                note_path: fact_id.into(),
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "aleph://user/preferences/language".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
            },
            EventActor::Agent,
            None,
        );
        env.timestamp = ts;
        env
    }

    /// Helper: build a NoteContentUpdated envelope.
    fn make_content_updated(
        fact_id: &str,
        seq: u64,
        ts: i64,
        new_content: &str,
    ) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::NoteContentUpdated {
                note_path: fact_id.into(),
                old_content: "User prefers Rust".into(),
                new_content: new_content.into(),
                reason: "correction".into(),
            },
            EventActor::User,
            None,
        );
        env.timestamp = ts;
        env
    }

    /// Helper: build a NoteInvalidated envelope.
    fn make_invalidated(fact_id: &str, seq: u64, ts: i64) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::NoteInvalidated {
                note_path: fact_id.into(),
                reason: "outdated".into(),
                actor: EventActor::System,
            },
            EventActor::System,
            None,
        );
        env.timestamp = ts;
        env
    }

    /// Helper: build a NoteAccessed envelope.
    fn make_accessed(fact_id: &str, seq: u64, ts: i64, count: u32) -> MemoryEventEnvelope {
        let mut env = MemoryEventEnvelope::new(
            fact_id.into(),
            seq,
            MemoryEvent::NoteAccessed {
                note_path: fact_id.into(),
                query: Some("rust preference".into()),
                relevance_score: Some(0.95),
                used_in_response: true,
                new_access_count: count,
            },
            EventActor::Agent,
            None,
        );
        env.timestamp = ts;
        env
    }

    // --- explain_fact --------------------------------------------------------

    #[tokio::test]
    async fn test_explain_fact_basic_lifecycle() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-1", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_content_updated(
            "fact-ex-1",
            2,
            2000,
            "User prefers Rust and Go",
        ))
        .await
        .unwrap();
        db.append_memory_event(&make_invalidated("fact-ex-1", 3, 3000))
            .await
            .unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-1").await.unwrap();

        assert_eq!(explanation.fact_id, "fact-ex-1");
        assert_eq!(explanation.events.len(), 3);
        assert!(!explanation.is_valid);
        assert_eq!(explanation.invalidation_reason.as_deref(), Some("outdated"));
        assert!(explanation.creation_source.is_some());
        assert_eq!(explanation.access_count, 0);

        // Check event descriptions
        assert!(explanation.events[0].action.contains("NoteCreated"));
        assert!(explanation.events[1].action.contains("NoteContentUpdated"));
        assert!(explanation.events[2].action.contains("NoteInvalidated"));
    }

    #[tokio::test]
    async fn test_explain_fact_tracks_access_count() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-2", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_accessed("fact-ex-2", 2, 2000, 1))
            .await
            .unwrap();
        db.append_memory_event(&make_accessed("fact-ex-2", 3, 3000, 2))
            .await
            .unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-2").await.unwrap();

        assert_eq!(explanation.access_count, 2);
        assert!(explanation.is_valid);
        assert_eq!(explanation.content.as_deref(), Some("User prefers Rust"));
    }

    #[tokio::test]
    async fn test_explain_fact_deleted_fact() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-3", 1, 1000))
            .await
            .unwrap();

        let mut del_env = MemoryEventEnvelope::new(
            "fact-ex-3".into(),
            2,
            MemoryEvent::NoteDeleted {
                note_path: "fact-ex-3".into(),
                reason: "user requested".into(),
            },
            EventActor::User,
            None,
        );
        del_env.timestamp = 2000;
        db.append_memory_event(&del_env).await.unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-3").await.unwrap();

        assert_eq!(explanation.fact_id, "fact-ex-3");
        assert!(!explanation.is_valid);
        assert!(explanation.content.is_none()); // deleted facts have no content
        assert_eq!(explanation.events.len(), 2);
    }

    #[tokio::test]
    async fn test_explain_nonexistent_fact() {
        let db = make_db();
        let traveler = MemoryTimeTraveler::new(db);
        let result = traveler.explain_fact("ghost").await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No events found"));
    }

    #[tokio::test]
    async fn test_explain_fact_invalidated_then_restored() {
        let db = make_db();
        db.append_memory_event(&make_created("fact-ex-4", 1, 1000))
            .await
            .unwrap();
        db.append_memory_event(&make_invalidated("fact-ex-4", 2, 2000))
            .await
            .unwrap();

        let mut restore_env = MemoryEventEnvelope::new(
            "fact-ex-4".into(),
            3,
            MemoryEvent::NoteRestored {
                note_path: "fact-ex-4".into(),
            },
            EventActor::User,
            None,
        );
        restore_env.timestamp = 3000;
        db.append_memory_event(&restore_env).await.unwrap();

        let traveler = MemoryTimeTraveler::new(db);
        let explanation = traveler.explain_fact("fact-ex-4").await.unwrap();

        assert!(explanation.is_valid);
        // Restoration clears invalidation_reason
        assert!(explanation.invalidation_reason.is_none());
        assert_eq!(explanation.events.len(), 3);
    }

    // --- describe_event (unit) -----------------------------------------------

    #[test]
    fn test_describe_event_all_variants() {
        // NoteCreated
        let env = make_created("f", 1, 1000);
        let desc = describe_event(&env);
        assert!(desc.contains("Fact created"));

        // NoteContentUpdated
        let env = make_content_updated("f", 2, 2000, "new stuff");
        let desc = describe_event(&env);
        assert!(desc.contains("Content updated"));
        assert!(desc.contains("correction"));

        // NoteInvalidated
        let env = make_invalidated("f", 3, 3000);
        let desc = describe_event(&env);
        assert!(desc.contains("Invalidated"));
        assert!(desc.contains("outdated"));
    }
}
