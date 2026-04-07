//! End-to-end integration tests for the LanceDB memory backend.
//!
//! Covers the full lifecycle:
//! 1. Backend creation
//! 2. Fact CRUD with embeddings
//! 3. FTS index creation via `ensure_indexes()`
//! 4. Vector search, text search, and hybrid search
//! 5. Graph operations (node upsert, retrieval)
//! 6. Stats consistency

use crate::memory::context::{FactType, MemoryFact};
use crate::memory::store::lance::LanceMemoryBackend;
use crate::memory::store::types::SearchFilter;
use crate::memory::store::{GraphNode, GraphStore, MemoryStore};

#[tokio::test]
async fn test_full_memory_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = LanceMemoryBackend::open_or_create(tmp.path())
        .await
        .unwrap();

    // -----------------------------------------------------------------------
    // 1. Insert facts with embeddings
    // -----------------------------------------------------------------------
    let embedding_a = vec![0.5f32; 1024];
    let mut fact1 = MemoryFact::new(
        "Aleph uses WebSocket for gateway".to_string(),
        FactType::Project,
        vec![],
    );
    fact1.embedding = Some(embedding_a.clone());
    fact1.embedding_model = "test-model".to_string();
    fact1.content_hash = "hash-1".to_string();
    backend.insert_fact(&fact1).await.unwrap();

    let mut fact2 = MemoryFact::new(
        "Rust is used for the core system".to_string(),
        FactType::Learning,
        vec![],
    );
    fact2.embedding = Some(vec![0.3f32; 1024]);
    fact2.embedding_model = "test-model".to_string();
    fact2.content_hash = "hash-2".to_string();
    backend.insert_fact(&fact2).await.unwrap();

    // -----------------------------------------------------------------------
    // 2. Build FTS indexes (requires data to exist)
    // -----------------------------------------------------------------------
    // Insert a graph node so the nodes table has data for FTS indexing
    let node = GraphNode {
        id: "aleph".to_string(),
        name: "Aleph".to_string(),
        kind: "project".to_string(),
        aliases: vec!["aleph-ai".to_string()],
        metadata_json: "{}".to_string(),
        decay_score: 1.0,
        created_at: 1700000000,
        updated_at: 1700000000,
        agent: "default".to_string(),
    };
    backend.upsert_node(&node, "default").await.unwrap();

    // Now build indexes — tables have data
    backend.ensure_indexes().await.unwrap();

    // -----------------------------------------------------------------------
    // 3. Vector search — verify results
    // -----------------------------------------------------------------------
    let results = backend
        .vector_search(&embedding_a, 1024, &SearchFilter::new(), 10)
        .await
        .unwrap();
    assert!(!results.is_empty(), "vector search should return results");
    // The closest match to embedding_a ([0.5; 1024]) should be fact1
    assert_eq!(
        results[0].fact.content, "Aleph uses WebSocket for gateway",
        "closest vector match should be fact1"
    );
    assert!(results[0].score > 0.0, "score should be positive");

    // -----------------------------------------------------------------------
    // 4. Text search (FTS) — verify results
    // -----------------------------------------------------------------------
    let text_results = backend
        .text_search("WebSocket", &SearchFilter::new(), 10)
        .await
        .unwrap();
    assert!(
        !text_results.is_empty(),
        "FTS search for 'WebSocket' should return results after index creation"
    );
    assert_eq!(
        text_results[0].fact.content,
        "Aleph uses WebSocket for gateway"
    );

    // -----------------------------------------------------------------------
    // 5. Graph operations — verify node retrieval
    // -----------------------------------------------------------------------
    let retrieved = backend.get_node("aleph", "default").await.unwrap();
    assert!(retrieved.is_some(), "node 'aleph' should exist");
    let retrieved_node = retrieved.unwrap();
    assert_eq!(retrieved_node.name, "Aleph");
    assert_eq!(retrieved_node.kind, "project");
    assert_eq!(retrieved_node.aliases, vec!["aleph-ai".to_string()]);

    // -----------------------------------------------------------------------
    // 6. Fact stats
    // -----------------------------------------------------------------------
    let stats = backend.get_fact_stats().await.unwrap();
    assert_eq!(stats.total_facts, 2, "should have 2 facts");
    assert_eq!(stats.valid_facts, 2, "both facts should be valid");

    // -----------------------------------------------------------------------
    // 7. Additional fact operations — update and invalidate
    // -----------------------------------------------------------------------
    backend
        .update_fact_content(&fact1.id, "Aleph uses WebSocket + JSON-RPC for gateway")
        .await
        .unwrap();
    let updated = backend.get_fact(&fact1.id).await.unwrap().unwrap();
    assert_eq!(
        updated.content,
        "Aleph uses WebSocket + JSON-RPC for gateway"
    );

    backend
        .invalidate_fact(&fact2.id, "superseded by new info")
        .await
        .unwrap();
    let invalidated = backend.get_fact(&fact2.id).await.unwrap().unwrap();
    assert!(!invalidated.is_valid);
    assert_eq!(
        invalidated.invalidation_reason,
        Some("superseded by new info".to_string())
    );

    // Verify stats after mutations
    let stats_after = backend.get_fact_stats().await.unwrap();
    assert_eq!(stats_after.total_facts, 2, "total facts unchanged");
    assert_eq!(stats_after.valid_facts, 1, "only 1 valid fact remains");
}

#[tokio::test]
async fn test_ensure_indexes_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = LanceMemoryBackend::open_or_create(tmp.path())
        .await
        .unwrap();

    // Insert minimum data so index creation succeeds
    let mut fact = MemoryFact::new("Test fact".to_string(), FactType::Other, vec![]);
    fact.embedding = Some(vec![0.1f32; 1024]);
    fact.content_hash = "hash-test".to_string();
    fact.embedding_model = "test".to_string();
    backend.insert_fact(&fact).await.unwrap();

    let node = GraphNode {
        id: "test-node".to_string(),
        name: "Test".to_string(),
        kind: "concept".to_string(),
        aliases: vec![],
        metadata_json: "{}".to_string(),
        decay_score: 1.0,
        created_at: 0,
        updated_at: 0,
        agent: "default".to_string(),
    };
    backend.upsert_node(&node, "default").await.unwrap();

    // First call
    backend.ensure_indexes().await.unwrap();
    // Second call — should be idempotent (replace=true)
    backend.ensure_indexes().await.unwrap();
}
