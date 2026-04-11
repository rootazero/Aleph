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
            enable_tunnels: true,
            max_tunnel_hops: 1,
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
// Graph-Augmented Retrieval — end-to-end integration test
// =============================================================================

#[cfg(test)]
mod graph_augmented_retrieval {
    use crate::memory::context::FactType;
    use crate::memory::context::MemoryFact;
    use crate::memory::hybrid_retrieval::graph_expander::{GraphExpander, GraphExpansionConfig};
    use crate::memory::store::types::ScoredFact;
    use crate::memory::store::{GraphEdge, GraphNode, GraphStore, MemoryStore, SqliteMemoryBackend};
    use crate::sync_primitives::Arc;

    #[tokio::test]
    async fn test_graph_augmented_retrieval_flow() {
        // 1. Set up in-memory test backend
        let backend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());

        // 2. Create two facts about related topics
        let fact_a = MemoryFact::new(
            "The user prefers Rust for systems programming".to_string(),
            FactType::Preference,
            vec![],
        );
        let fact_b = MemoryFact::new(
            "Aleph is built with Rust and uses axum".to_string(),
            FactType::Project,
            vec![],
        );

        backend.insert_fact(&fact_a).await.unwrap();
        backend.insert_fact(&fact_b).await.unwrap();

        // 3. Create graph node "Rust" and link both facts to it
        let rust_node = GraphNode {
            id: "gn-rust".to_string(),
            name: "Rust".to_string(),
            kind: "technology".to_string(),
            aliases: vec![],
            metadata_json: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            agent: "default".to_string(),
        };
        backend.upsert_node(&rust_node, "default").await.unwrap();

        backend
            .link_memory_entity(&fact_a.id, "gn-rust", 0.8, "extracted", "default")
            .await
            .unwrap();
        backend
            .link_memory_entity(&fact_b.id, "gn-rust", 0.9, "extracted", "default")
            .await
            .unwrap();

        // 4. Verify bidirectional lookups
        let nodes = backend
            .get_nodes_for_fact(&fact_a.id, "default")
            .await
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].0.name, "Rust");

        let facts = backend
            .get_facts_for_node("gn-rust", "default")
            .await
            .unwrap();
        assert_eq!(facts.len(), 2);

        // 5. Create a second node and edge so fact_b is reachable from fact_a via graph traversal:
        //    fact_a → gn-rust --[used_by]--> gn-aleph → fact_b
        let aleph_node = GraphNode {
            id: "gn-aleph".to_string(),
            name: "Aleph".to_string(),
            kind: "project".to_string(),
            aliases: vec![],
            metadata_json: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            agent: "default".to_string(),
        };
        backend.upsert_node(&aleph_node, "default").await.unwrap();
        backend
            .link_memory_entity(&fact_b.id, "gn-aleph", 0.9, "extracted", "default")
            .await
            .unwrap();

        let edge = GraphEdge {
            id: "ge-rust-aleph".to_string(),
            from_id: "gn-rust".to_string(),
            to_id: "gn-aleph".to_string(),
            relation: "used_by".to_string(),
            weight: 1.0,
            confidence: 0.9,
            context_key: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            last_seen_at: 1700000000,
            agent: "default".to_string(),
        };
        backend.upsert_edge(&edge, "default").await.unwrap();

        // 6. Run GraphExpander seeded with fact_a only; fact_b should be discovered
        let seeds = vec![ScoredFact {
            fact: fact_a.clone(),
            score: 0.9,
        }];

        let expander = GraphExpander::new(backend.clone(), GraphExpansionConfig::default());
        let expanded = expander.expand(&seeds, "default").await.unwrap();

        // fact_b should be discovered via: fact_a → gn-rust → used_by → gn-aleph → fact_b
        assert!(
            !expanded.is_empty(),
            "Should discover fact_b via graph expansion"
        );
        assert_eq!(expanded[0].scored_fact.fact.id, fact_b.id);
        assert!(
            expanded[0].scored_fact.score < 0.9,
            "Expanded score ({}) should be lower than seed score (0.9)",
            expanded[0].scored_fact.score
        );
    }
}

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
        let handler = MemoryCommandHandler::new(db.clone(), None);
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
                agent: "default".into(),
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
