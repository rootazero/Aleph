//! Integration tests for Memory System Evolution
//!
//! These tests verify that memory system components can be instantiated
//! and configured correctly, and that the event sourcing subsystem works
//! end-to-end.
//!
//! Note: Most tests are marked as #[ignore] because they require model downloads.
//!
//! Run with: cargo test --lib memory::integration_tests -- --ignored

pub mod workspace_isolation;

#[cfg(test)]
#[allow(clippy::module_inception)]
mod integration_tests {
    use crate::memory::{
        context_comptroller::{ComptrollerConfig, RetentionMode},
        ripple::RippleConfig,
    };

    #[tokio::test]
    async fn test_comptroller_config() {
        // Test that ComptrollerConfig can be created
        let config = ComptrollerConfig {
            similarity_threshold: 0.95,
            token_budget: 1000,
            fold_threshold: 0.2,
            retention_mode: RetentionMode::Hybrid,
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
    async fn test_retention_modes() {
        // Test that all retention modes are available
        let modes = vec![
            RetentionMode::PreferTranscript,
            RetentionMode::PreferFact,
            RetentionMode::Hybrid,
        ];

        assert_eq!(modes.len(), 3, "Should have 3 retention modes");
        println!("Available retention modes: {:?}", modes);
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

// =============================================================================
// Event Sourcing — full round-trip integration test
// =============================================================================

#[cfg(test)]
mod event_sourcing {
    use std::sync::Arc;

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
            .create_fact(CreateFactCommand {
                content: "User prefers Rust for systems programming".into(),
                fact_type: FactType::Preference,
                tier: MemoryTier::ShortTerm,
                scope: MemoryScope::Global,
                path: "/user/preferences/language".into(),
                namespace: "owner".into(),
                workspace: "default".into(),
                confidence: 0.9,
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
        assert_eq!(events[0].event.event_type_tag(), "FactCreated");
        assert_eq!(events[0].seq, 1);

        // Rebuild from events
        let fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .unwrap();
        assert_eq!(fact.content, "User prefers Rust for systems programming");
        assert_eq!(fact.tier, MemoryTier::ShortTerm);

        // 2. Update content
        handler
            .update_content(UpdateContentCommand {
                fact_id: fact_id.clone(),
                new_content: "User strongly prefers Rust for all programming".into(),
                reason: "User reinforced preference".into(),
                actor: EventActor::Agent,
                correlation_id: Some("session-43".into()),
            })
            .await
            .unwrap();

        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].event.event_type_tag(), "FactContentUpdated");

        // 3. Record access (Pulse)
        handler
            .record_access(RecordAccessCommand {
                fact_id: fact_id.clone(),
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
            .invalidate_fact(InvalidateFactCommand {
                fact_id: fact_id.clone(),
                reason: "Contradicted by newer information".into(),
                actor: EventActor::System,
                strength_at_invalidation: Some(0.8),
                correlation_id: None,
            })
            .await
            .unwrap();

        // 5. Restore
        handler
            .restore_fact(RestoreFactCommand {
                fact_id: fact_id.clone(),
                new_strength: 0.7,
                correlation_id: None,
            })
            .await
            .unwrap();

        // Verify full event trail
        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 5);

        // 6. Verify final state via projector
        let final_fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .unwrap();
        assert_eq!(
            final_fact.content,
            "User strongly prefers Rust for all programming"
        );
        assert!(final_fact.is_valid);
        assert_eq!(final_fact.strength, 0.7);
        assert_eq!(final_fact.access_count, 1);

        // 7. Time travel -- verify full timeline via traveler
        let timeline = traveler.fact_timeline(&fact_id).await.unwrap();
        assert_eq!(timeline.len(), 5);

        // 8. Explain fact
        let explanation = traveler.explain_fact(&fact_id).await.unwrap();
        assert_eq!(explanation.fact_id, fact_id);
        assert_eq!(explanation.events.len(), 5);
        // First event should describe creation
        assert!(explanation.events[0].action.contains("FactCreated"));

        // 9. Test decay
        let decay_count = handler
            .apply_decay(ApplyDecayCommand {
                fact_ids_with_strength: vec![(fact_id.clone(), 0.7, 0.65)],
                decay_factor: 0.95,
                correlation_id: None,
            })
            .await
            .unwrap();
        assert_eq!(decay_count, 1);

        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 6);
        let final_fact = EventProjector::fold_events_to_fact(&events)
            .unwrap()
            .unwrap();
        assert_eq!(final_fact.strength, 0.65);

        // 10. Delete
        handler
            .delete_fact(DeleteFactCommand {
                fact_id: fact_id.clone(),
                reason: "User requested removal".into(),
                actor: EventActor::User,
                correlation_id: None,
            })
            .await
            .unwrap();

        let events = db.get_memory_events_for_fact(&fact_id).await.unwrap();
        assert_eq!(events.len(), 7);
        let deleted = EventProjector::fold_events_to_fact(&events).unwrap();
        assert!(deleted.is_none()); // Fact deleted
    }
}
