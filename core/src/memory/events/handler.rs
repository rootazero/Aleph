//! MemoryCommandHandler — write-side facade for event-sourced memory mutations.
//!
//! All fact mutations go through this handler:
//! 1. Build MemoryEvent from command
//! 2. Append to SQLite event store
//!
//! Projection to LanceDB happens downstream.

use crate::sync_primitives::Arc;
use uuid::Uuid;

use crate::error::AlephError;
use crate::memory::events::{EventActor, MemoryEvent, MemoryEventEnvelope};
use crate::resilience::database::StateDatabase;

use super::commands::*;

pub struct MemoryCommandHandler {
    db: Arc<StateDatabase>,
}

impl MemoryCommandHandler {
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self { db }
    }

    /// Create a new fact. Returns the generated fact_id.
    pub async fn create_fact(&self, cmd: CreateFactCommand) -> Result<String, AlephError> {
        let fact_id = Uuid::new_v4().to_string();
        let seq = self.db.get_memory_event_latest_seq(&fact_id).await? + 1;

        let event = MemoryEvent::FactCreated {
            fact_id: fact_id.clone(),
            content: cmd.content,
            fact_type: cmd.fact_type,
            tier: cmd.tier,
            scope: cmd.scope,
            path: cmd.path,
            namespace: cmd.namespace,
            workspace: cmd.workspace,
            confidence: cmd.confidence,
            source: cmd.source,
            source_memory_ids: cmd.source_memory_ids,
        };

        let envelope = MemoryEventEnvelope::new(
            fact_id.clone(),
            seq,
            event,
            cmd.actor,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        Ok(fact_id)
    }

    /// Update the content of an existing fact.
    pub async fn update_content(&self, cmd: UpdateContentCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.fact_id).await? + 1;

        // Rebuild from events to get the current content.
        let events = self.db.get_memory_events_for_fact(&cmd.fact_id).await?;
        let current_fact = super::projector::EventProjector::fold_events_to_fact(&events)?
            .ok_or_else(|| {
                AlephError::other(format!("Fact {} not found or deleted", cmd.fact_id))
            })?;

        let event = MemoryEvent::FactContentUpdated {
            fact_id: cmd.fact_id.clone(),
            old_content: current_fact.content,
            new_content: cmd.new_content,
            reason: cmd.reason,
        };

        let envelope = MemoryEventEnvelope::new(
            cmd.fact_id,
            seq,
            event,
            cmd.actor,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        Ok(())
    }

    /// Invalidate (soft-delete) a fact.
    pub async fn invalidate_fact(&self, cmd: InvalidateFactCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.fact_id).await? + 1;

        let event = MemoryEvent::FactInvalidated {
            fact_id: cmd.fact_id.clone(),
            reason: cmd.reason,
            actor: cmd.actor.clone(),
            strength_at_invalidation: cmd.strength_at_invalidation,
        };

        let envelope = MemoryEventEnvelope::new(
            cmd.fact_id,
            seq,
            event,
            cmd.actor,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        Ok(())
    }

    /// Restore a previously invalidated fact.
    pub async fn restore_fact(&self, cmd: RestoreFactCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.fact_id).await? + 1;

        let event = MemoryEvent::FactRestored {
            fact_id: cmd.fact_id.clone(),
            new_strength: cmd.new_strength,
        };

        let envelope = MemoryEventEnvelope::new(
            cmd.fact_id,
            seq,
            event,
            EventActor::User,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        Ok(())
    }

    /// Record a fact access (Pulse event).
    pub async fn record_access(&self, cmd: RecordAccessCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.fact_id).await? + 1;

        // Get current access count from event history
        let events = self.db.get_memory_events_for_fact(&cmd.fact_id).await?;
        let current_fact = super::projector::EventProjector::fold_events_to_fact(&events)?;
        let current_access_count = current_fact.map(|f| f.access_count).unwrap_or(0);

        let event = MemoryEvent::FactAccessed {
            fact_id: cmd.fact_id.clone(),
            query: cmd.query,
            relevance_score: cmd.relevance_score,
            used_in_response: cmd.used_in_response,
            new_access_count: current_access_count + 1,
        };

        let envelope = MemoryEventEnvelope::new(
            cmd.fact_id,
            seq,
            event,
            EventActor::Agent,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        Ok(())
    }

    /// Apply strength decay to multiple facts (bulk Pulse events).
    pub async fn apply_decay(&self, cmd: ApplyDecayCommand) -> Result<usize, AlephError> {
        let mut envelopes = Vec::with_capacity(cmd.fact_ids_with_strength.len());

        for (fact_id, old_strength, new_strength) in &cmd.fact_ids_with_strength {
            let seq = self.db.get_memory_event_latest_seq(fact_id).await? + 1;
            let event = MemoryEvent::StrengthDecayed {
                fact_id: fact_id.clone(),
                old_strength: *old_strength,
                new_strength: *new_strength,
                decay_factor: cmd.decay_factor,
            };
            envelopes.push(MemoryEventEnvelope::new(
                fact_id.clone(),
                seq,
                event,
                EventActor::Decay,
                cmd.correlation_id.clone(),
            ));
        }

        let count = envelopes.len();
        self.db.append_memory_events(&envelopes).await?;
        Ok(count)
    }

    /// Consolidate multiple facts into one new fact.
    pub async fn consolidate_facts(&self, cmd: ConsolidateCommand) -> Result<String, AlephError> {
        let fact_id = Uuid::new_v4().to_string();
        let seq = 1u64; // New fact, starts at seq 1

        let event = MemoryEvent::FactConsolidated {
            fact_id: fact_id.clone(),
            source_fact_ids: cmd.source_fact_ids,
            consolidated_content: cmd.consolidated_content,
        };

        let envelope = MemoryEventEnvelope::new(
            fact_id.clone(),
            seq,
            event,
            cmd.actor,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        Ok(fact_id)
    }

    /// Permanently delete a fact.
    pub async fn delete_fact(&self, cmd: DeleteFactCommand) -> Result<(), AlephError> {
        let seq = self.db.get_memory_event_latest_seq(&cmd.fact_id).await? + 1;

        let event = MemoryEvent::FactDeleted {
            fact_id: cmd.fact_id.clone(),
            reason: cmd.reason,
        };

        let envelope = MemoryEventEnvelope::new(
            cmd.fact_id,
            seq,
            event,
            cmd.actor,
            cmd.correlation_id,
        );

        self.db.append_memory_event(&envelope).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, FactType, MemoryScope, MemoryTier};
    use crate::memory::events::projector::EventProjector;

    fn make_handler() -> MemoryCommandHandler {
        let db = Arc::new(crate::resilience::database::StateDatabase::in_memory().unwrap());
        MemoryCommandHandler::new(db)
    }

    /// Helper: create a fact and return (handler, fact_id)
    async fn make_handler_with_fact() -> (MemoryCommandHandler, String) {
        let handler = make_handler();
        let fact_id = handler
            .create_fact(CreateFactCommand {
                content: "User prefers Rust".into(),
                fact_type: FactType::Preference,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                path: "/user/preferences".into(),
                namespace: "owner".into(),
                workspace: "default".into(),
                confidence: 0.9,
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();
        (handler, fact_id)
    }

    #[tokio::test]
    async fn test_create_fact() {
        let handler = make_handler();
        let fact_id = handler
            .create_fact(CreateFactCommand {
                content: "User prefers Rust".into(),
                fact_type: FactType::Preference,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                path: "/user/preferences".into(),
                namespace: "owner".into(),
                workspace: "default".into(),
                confidence: 0.9,
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        assert!(!fact_id.is_empty());

        // Verify event was stored
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.event_type_tag(), "FactCreated");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].actor, EventActor::Agent);

        // Verify fact can be projected
        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");
        assert_eq!(fact.id, fact_id);
        assert_eq!(fact.content, "User prefers Rust");
        assert_eq!(fact.fact_type, FactType::Preference);
        assert_eq!(fact.tier, MemoryTier::ShortTerm);
    }

    #[tokio::test]
    async fn test_update_content() {
        let (handler, fact_id) = make_handler_with_fact().await;

        handler
            .update_content(UpdateContentCommand {
                fact_id: fact_id.clone(),
                new_content: "User prefers Rust and Go".into(),
                reason: "correction".into(),
                actor: EventActor::User,
                correlation_id: Some("session-42".into()),
            })
            .await
            .unwrap();

        // Verify two events stored
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event.event_type_tag(), "FactContentUpdated");
        assert_eq!(events[1].seq, 2);

        // Verify the old_content was captured correctly
        if let MemoryEvent::FactContentUpdated {
            old_content,
            new_content,
            reason,
            ..
        } = &events[1].event
        {
            assert_eq!(old_content, "User prefers Rust");
            assert_eq!(new_content, "User prefers Rust and Go");
            assert_eq!(reason, "correction");
        } else {
            panic!("Expected FactContentUpdated event");
        }

        // Verify projection
        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");
        assert_eq!(fact.content, "User prefers Rust and Go");
    }

    #[tokio::test]
    async fn test_update_content_nonexistent_fact_fails() {
        let handler = make_handler();
        let result = handler
            .update_content(UpdateContentCommand {
                fact_id: "nonexistent".into(),
                new_content: "new content".into(),
                reason: "test".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalidate_and_restore() {
        let (handler, fact_id) = make_handler_with_fact().await;

        // Invalidate
        handler
            .invalidate_fact(InvalidateFactCommand {
                fact_id: fact_id.clone(),
                reason: "outdated information".into(),
                actor: EventActor::User,
                strength_at_invalidation: Some(0.5),
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify invalidated state
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id)
            .await
            .unwrap();
        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");
        assert!(!fact.is_valid);
        assert_eq!(
            fact.invalidation_reason.as_deref(),
            Some("outdated information")
        );

        // Restore
        handler
            .restore_fact(RestoreFactCommand {
                fact_id: fact_id.clone(),
                new_strength: 0.7,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify restored state
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id)
            .await
            .unwrap();
        assert_eq!(events.len(), 3); // Created + Invalidated + Restored
        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");
        assert!(fact.is_valid);
        assert!(fact.invalidation_reason.is_none());
        assert!((fact.strength - 0.7).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_record_access_increments_count() {
        let (handler, fact_id) = make_handler_with_fact().await;

        // First access
        handler
            .record_access(RecordAccessCommand {
                fact_id: fact_id.clone(),
                query: Some("what language?".into()),
                relevance_score: Some(0.95),
                used_in_response: true,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Second access
        handler
            .record_access(RecordAccessCommand {
                fact_id: fact_id.clone(),
                query: None,
                relevance_score: None,
                used_in_response: false,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify access count
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id)
            .await
            .unwrap();
        assert_eq!(events.len(), 3); // Created + 2 Accessed
        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .expect("should produce a fact");
        assert_eq!(fact.access_count, 2);
        assert!(fact.last_accessed_at.is_some());
    }

    #[tokio::test]
    async fn test_apply_decay_bulk() {
        let handler = make_handler();

        // Create 3 facts
        let mut fact_ids = Vec::new();
        for i in 0..3 {
            let fid = handler
                .create_fact(CreateFactCommand {
                    content: format!("Fact {i}"),
                    fact_type: FactType::Learning,
                    tier: MemoryTier::ShortTerm,
                    scope: MemoryScope::Global,
                    path: format!("/test/fact{i}"),
                    namespace: "owner".into(),
                    workspace: "default".into(),
                    confidence: 0.8,
                    source: FactSource::Extracted,
                    source_memory_ids: vec![],
                    actor: EventActor::Agent,
                    correlation_id: None,
                })
                .await
                .unwrap();
            fact_ids.push(fid);
        }

        // Apply decay to all 3
        let decay_data: Vec<(String, f32, f32)> = fact_ids
            .iter()
            .map(|id| (id.clone(), 1.0, 0.95))
            .collect();

        let count = handler
            .apply_decay(ApplyDecayCommand {
                fact_ids_with_strength: decay_data,
                decay_factor: 0.95,
                correlation_id: Some("decay-batch-1".into()),
            })
            .await
            .unwrap();

        assert_eq!(count, 3);

        // Verify each fact has a decay event
        for fact_id in &fact_ids {
            let events = handler
                .db
                .get_memory_events_for_fact(fact_id)
                .await
                .unwrap();
            assert_eq!(events.len(), 2); // Created + StrengthDecayed
            assert_eq!(events[1].event.event_type_tag(), "StrengthDecayed");
            assert_eq!(events[1].actor, EventActor::Decay);

            let fact = EventProjector::fold_events_to_fact(&events)
                .unwrap()
                .expect("should produce a fact");
            assert!((fact.strength - 0.95).abs() < f32::EPSILON);
        }
    }

    #[tokio::test]
    async fn test_delete_fact() {
        let (handler, fact_id) = make_handler_with_fact().await;

        handler
            .delete_fact(DeleteFactCommand {
                fact_id: fact_id.clone(),
                reason: "user requested removal".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify events stored
        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id)
            .await
            .unwrap();
        assert_eq!(events.len(), 2); // Created + Deleted
        assert_eq!(events[1].event.event_type_tag(), "FactDeleted");

        // Verify projection returns None (deleted fact)
        let fact = EventProjector::fold_events_to_fact(&events).unwrap();
        assert!(fact.is_none());
    }

    #[tokio::test]
    async fn test_consolidate_facts() {
        let handler = make_handler();

        // Create two source facts
        let fid1 = handler
            .create_fact(CreateFactCommand {
                content: "User likes Rust".into(),
                fact_type: FactType::Preference,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                path: "/user/preferences/lang1".into(),
                namespace: "owner".into(),
                workspace: "default".into(),
                confidence: 0.8,
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        let fid2 = handler
            .create_fact(CreateFactCommand {
                content: "User likes Go".into(),
                fact_type: FactType::Preference,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                path: "/user/preferences/lang2".into(),
                namespace: "owner".into(),
                workspace: "default".into(),
                confidence: 0.7,
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Consolidate
        let consolidated_id = handler
            .consolidate_facts(ConsolidateCommand {
                source_fact_ids: vec![fid1.clone(), fid2.clone()],
                consolidated_content: "User likes both Rust and Go".into(),
                actor: EventActor::System,
                correlation_id: Some("consolidation-1".into()),
            })
            .await
            .unwrap();

        assert!(!consolidated_id.is_empty());
        assert_ne!(consolidated_id, fid1);
        assert_ne!(consolidated_id, fid2);

        // Verify consolidated event stored
        let events = handler
            .db
            .get_memory_events_for_fact(&consolidated_id)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.event_type_tag(), "FactConsolidated");
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].actor, EventActor::System);

        if let MemoryEvent::FactConsolidated {
            source_fact_ids,
            consolidated_content,
            ..
        } = &events[0].event
        {
            assert_eq!(source_fact_ids.len(), 2);
            assert!(source_fact_ids.contains(&fid1));
            assert!(source_fact_ids.contains(&fid2));
            assert_eq!(consolidated_content, "User likes both Rust and Go");
        } else {
            panic!("Expected FactConsolidated event");
        }
    }

    #[tokio::test]
    async fn test_seq_increments_correctly() {
        let (handler, fact_id) = make_handler_with_fact().await;

        // Perform multiple operations on the same fact
        handler
            .record_access(RecordAccessCommand {
                fact_id: fact_id.clone(),
                query: None,
                relevance_score: None,
                used_in_response: false,
                correlation_id: None,
            })
            .await
            .unwrap();

        handler
            .update_content(UpdateContentCommand {
                fact_id: fact_id.clone(),
                new_content: "Updated content".into(),
                reason: "test".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        let events = handler
            .db
            .get_memory_events_for_fact(&fact_id)
            .await
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1); // Created
        assert_eq!(events[1].seq, 2); // Accessed
        assert_eq!(events[2].seq, 3); // ContentUpdated
    }
}
