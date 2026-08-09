//! Integration tests for Memory System Evolution
//!
//! These tests verify that memory system components can be instantiated
//! and configured correctly, and that the event sourcing subsystem works
//! end-to-end.
//!
//! Note: Most tests are marked as #[ignore] because they require model downloads.
//!
//! Run with: cargo test --lib memory::integration_tests -- --ignored

#[cfg(test)]
#[allow(clippy::module_inception)]
mod integration_tests {
    use crate::memory::{context_comptroller::ComptrollerConfig, ripple::RippleConfig};

    #[tokio::test]
    async fn test_comptroller_config() {
        // Test that ComptrollerConfig can be created
        let config = ComptrollerConfig {
            similarity_threshold: 0.95,
            token_budget: 1000,
            fold_threshold: 0.2,
        };

        assert_eq!(config.similarity_threshold, 0.95);
        assert_eq!(config.token_budget, 1000);
        println!("ComptrollerConfig created: {:?}", config);
    }

    #[tokio::test]
    async fn test_ripple_config() {
        // Test RippleTask configuration
        let config = RippleConfig {
            max_hops: 3,
            max_facts_per_hop: 5,
            similarity_threshold: 0.7,
        };

        assert_eq!(config.max_hops, 3);
        assert_eq!(config.max_facts_per_hop, 5);
        assert_eq!(config.similarity_threshold, 0.7);
        println!("RippleConfig created: {:?}", config);
    }

    #[tokio::test]
    async fn test_default_config() {
        // Test default configuration
        let config = ComptrollerConfig::default();

        assert_eq!(config.similarity_threshold, 0.95);
        assert_eq!(config.token_budget, 100000);
        assert_eq!(config.fold_threshold, 0.2);
        println!("Default config: {:?}", config);
    }
}

// NOTE: Graph-Augmented Retrieval integration tests have been removed.
// The graph_nodes/graph_edges/memory_entities system is deprecated.
// Knowledge Notes with wikilink-based linking are the replacement.

// =============================================================================
// Event Sourcing — full round-trip integration test
// =============================================================================

#[cfg(test)]
mod event_sourcing {
    use crate::sync_primitives::Arc;

    use crate::memory::context::*;
    use crate::memory::events::commands::*;
    use crate::memory::events::handler::MemoryCommandHandler;
    use crate::memory::events::projector::EventProjector;
    use crate::memory::events::traveler::MemoryTimeTraveler;
    use crate::memory::events::*;
    use crate::resilience::database::StateDatabase;

    /// Full round-trip: create -> update -> access -> invalidate -> restore
    /// -> decay -> delete, verifying event trail, projector, traveler, and
    /// explain at each stage.
    #[tokio::test]
    async fn test_event_sourcing_full_round_trip() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let handler = MemoryCommandHandler::new(db.clone());
        let traveler = MemoryTimeTraveler::new(db.clone());

        // 1. Create a fact
        let fact_id = handler
            .create_fact(CreateNoteCommand {
                content: "User prefers Rust for systems programming".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/language".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec!["conv-001".into()],
                actor: EventActor::Agent,
                correlation_id: Some("session-42".into()),
            })
            .await
            .unwrap();
        assert!(!fact_id.is_empty());

        // Verify event stored
        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.event_type_tag(), "NoteCreated");
        assert_eq!(events[0].seq, 1);

        // Rebuild from events
        let fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .unwrap();
        assert_eq!(fact.content, "User prefers Rust for systems programming");

        // 2. Update content
        handler
            .update_content(UpdateContentCommand {
                note_path: fact_id.clone(),
                new_content: "User strongly prefers Rust for all programming".into(),
                reason: "User reinforced preference".into(),
                actor: EventActor::Agent,
                correlation_id: Some("session-43".into()),
            })
            .await
            .unwrap();

        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event.event_type_tag(), "NoteContentUpdated");

        // 3. Record access (Pulse)
        handler
            .record_access(RecordNoteAccessCommand {
                note_path: fact_id.clone(),
                query: Some("What language does the user prefer?".into()),
                relevance_score: Some(0.95),
                used_in_response: true,
                correlation_id: None,
            })
            .await
            .unwrap();

        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 3);
        assert!(!events[2].is_skeleton()); // Pulse event

        // 4. Invalidate
        handler
            .invalidate_fact(InvalidateNoteCommand {
                note_path: fact_id.clone(),
                reason: "Contradicted by newer information".into(),
                actor: EventActor::System,
                correlation_id: None,
            })
            .await
            .unwrap();

        // 5. Restore
        handler
            .restore_fact(RestoreNoteCommand {
                note_path: fact_id.clone(),
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify full event trail
        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 5);

        // 6. Verify final state via projector
        let final_fact = EventProjector::fold_events_to_note(&events)
            .unwrap()
            .unwrap();
        assert_eq!(
            final_fact.content,
            "User strongly prefers Rust for all programming"
        );
        assert!(final_fact.is_valid);
        assert_eq!(final_fact.access_count, 1);

        // 7. The full event timeline is reachable via the event store
        let timeline = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(timeline.len(), 5);

        // 8. Explain fact
        let explanation = traveler.explain_fact(&fact_id).await.unwrap();
        assert_eq!(explanation.fact_id, fact_id);
        assert_eq!(explanation.events.len(), 5);
        // First event should describe creation
        assert!(explanation.events[0].action.contains("NoteCreated"));

        // 9. Delete
        handler
            .delete_fact(DeleteNoteCommand {
                note_path: fact_id.clone(),
                reason: "User requested removal".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 6);
        let deleted = EventProjector::fold_events_to_note(&events).unwrap();
        assert!(deleted.is_none()); // Fact deleted
    }
}
