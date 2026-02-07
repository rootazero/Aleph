//! Integration tests for memory namespace isolation
//!
//! Verifies that Owner and Guest namespaces are properly isolated at database layer.

use alephcore::memory::database::VectorDatabase;
use alephcore::memory::context::MemoryFact;
use alephcore::memory::NamespaceScope;
use tempfile::TempDir;

/// Helper: Create test database with sample facts
async fn create_test_db_with_facts() -> (VectorDatabase, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    // Create a simple test embedding (384-dim)
    let test_embedding = vec![1.0f32; 384];

    // Insert owner facts
    let owner_fact = MemoryFact {
        id: "owner-fact-1".to_string(),
        content: "Owner secret data".to_string(),
        fact_type: alephcore::memory::context::FactType::Other,
        embedding: Some(test_embedding.clone()),
        source_memory_ids: vec![],
        created_at: 1000,
        updated_at: 1000,
        confidence: 1.0,
        is_valid: true,
        invalidation_reason: None,
        specificity: alephcore::memory::context::FactSpecificity::Pattern,
        temporal_scope: alephcore::memory::context::TemporalScope::Contextual,
        decay_invalidated_at: None,
        similarity_score: None,
    };
    db.insert_fact_with_namespace(&owner_fact, NamespaceScope::Owner)
        .await
        .unwrap();

    // Insert guest facts
    let guest_fact = MemoryFact {
        id: "guest-fact-1".to_string(),
        content: "Guest alice data".to_string(),
        fact_type: alephcore::memory::context::FactType::Other,
        embedding: Some(test_embedding),
        source_memory_ids: vec![],
        created_at: 2000,
        updated_at: 2000,
        confidence: 1.0,
        is_valid: true,
        invalidation_reason: None,
        specificity: alephcore::memory::context::FactSpecificity::Pattern,
        temporal_scope: alephcore::memory::context::TemporalScope::Contextual,
        decay_invalidated_at: None,
        similarity_score: None,
    };
    db.insert_fact_with_namespace(&guest_fact, NamespaceScope::Guest("alice".into()))
        .await
        .unwrap();

    (db, temp_dir)
}

#[tokio::test]
async fn test_guest_cannot_read_owner_facts() {
    let (db, _temp) = create_test_db_with_facts().await;

    // Create same embedding as test data for search
    let query_embedding = vec![1.0f32; 384];

    // Guest search should only see their own facts
    let results = db
        .search_facts(&query_embedding, NamespaceScope::Guest("alice".into()), 10, false)
        .await
        .unwrap();

    // Should only see guest-fact-1, not owner-fact-1
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "guest-fact-1");
    assert!(!results.iter().any(|f| f.id == "owner-fact-1"));
}

#[tokio::test]
async fn test_owner_can_read_all_namespaces() {
    let (db, _temp) = create_test_db_with_facts().await;

    // Create same embedding as test data for search
    let query_embedding = vec![1.0f32; 384];

    // Owner search should see all facts
    let results = db
        .search_facts(&query_embedding, NamespaceScope::Owner, 10, false)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    let ids: Vec<&str> = results.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(&"owner-fact-1"));
    assert!(ids.contains(&"guest-fact-1"));
}

#[tokio::test]
async fn test_guests_cannot_see_each_other() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    // Create a simple test embedding (384-dim)
    let test_embedding = vec![1.0f32; 384];

    // Insert facts for two different guests
    let alice_fact = MemoryFact {
        id: "alice-fact".to_string(),
        content: "Alice data".to_string(),
        fact_type: alephcore::memory::context::FactType::Other,
        embedding: Some(test_embedding.clone()),
        source_memory_ids: vec![],
        created_at: 1000,
        updated_at: 1000,
        confidence: 1.0,
        is_valid: true,
        invalidation_reason: None,
        specificity: alephcore::memory::context::FactSpecificity::Pattern,
        temporal_scope: alephcore::memory::context::TemporalScope::Contextual,
        decay_invalidated_at: None,
        similarity_score: None,
    };
    db.insert_fact_with_namespace(&alice_fact, NamespaceScope::Guest("alice".into()))
        .await
        .unwrap();

    let bob_fact = MemoryFact {
        id: "bob-fact".to_string(),
        content: "Bob data".to_string(),
        fact_type: alephcore::memory::context::FactType::Other,
        embedding: Some(test_embedding.clone()),
        source_memory_ids: vec![],
        created_at: 2000,
        updated_at: 2000,
        confidence: 1.0,
        is_valid: true,
        invalidation_reason: None,
        specificity: alephcore::memory::context::FactSpecificity::Pattern,
        temporal_scope: alephcore::memory::context::TemporalScope::Contextual,
        decay_invalidated_at: None,
        similarity_score: None,
    };
    db.insert_fact_with_namespace(&bob_fact, NamespaceScope::Guest("bob".into()))
        .await
        .unwrap();

    // Create dummy embedding for search (same as what we inserted)
    let query_embedding = vec![1.0f32; 384];

    // Alice should only see her facts
    let alice_results = db
        .search_facts(&query_embedding, NamespaceScope::Guest("alice".into()), 10, false)
        .await
        .unwrap();
    assert_eq!(alice_results.len(), 1);
    assert_eq!(alice_results[0].id, "alice-fact");

    // Bob should only see his facts
    let bob_results = db
        .search_facts(&query_embedding, NamespaceScope::Guest("bob".into()), 10, false)
        .await
        .unwrap();
    assert_eq!(bob_results.len(), 1);
    assert_eq!(bob_results[0].id, "bob-fact");
}
