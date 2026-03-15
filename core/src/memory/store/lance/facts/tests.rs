use std::collections::HashSet;

use super::*;
use crate::memory::context::{FactType, MemoryFact};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::SearchFilter;
use crate::memory::store::MemoryStore;
use super::super::LanceMemoryBackend;

/// Helper: create a test LanceMemoryBackend in a temp directory.
async fn create_test_backend() -> (tempfile::TempDir, LanceMemoryBackend) {
    let tmp = tempfile::tempdir().unwrap();
    let backend = LanceMemoryBackend::open_or_create(tmp.path())
        .await
        .unwrap();
    (tmp, backend)
}

/// Helper: create a test fact with optional embedding.
fn make_test_fact(content: &str, fact_type: FactType, with_embedding: bool) -> MemoryFact {
    let mut fact = MemoryFact::new(
        content.to_string(),
        fact_type,
        vec!["mem-001".to_string()],
    );
    fact.confidence = 0.9;
    fact.content_hash = "hash123".to_string();
    fact.embedding_model = "test-model".to_string();
    if with_embedding {
        fact.embedding = Some(vec![0.1_f32; 1024]);
    }
    fact
}

#[tokio::test]
async fn test_insert_and_get_fact() {
    let (_tmp, backend) = create_test_backend().await;
    let fact = make_test_fact("Test content", FactType::Learning, false);
    let fact_id = fact.id.clone();

    backend.insert_fact(&fact).await.unwrap();

    let retrieved = backend.get_fact(&fact_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.content, "Test content");
    assert_eq!(retrieved.fact_type, FactType::Learning);
    assert_eq!(retrieved.id, fact_id);
}

#[tokio::test]
async fn test_get_nonexistent_fact() {
    let (_tmp, backend) = create_test_backend().await;
    let result = backend.get_fact("nonexistent-id").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_delete_fact() {
    let (_tmp, backend) = create_test_backend().await;
    let fact = make_test_fact("To be deleted", FactType::Other, false);
    let fact_id = fact.id.clone();

    backend.insert_fact(&fact).await.unwrap();
    assert!(backend.get_fact(&fact_id).await.unwrap().is_some());

    backend.delete_fact(&fact_id).await.unwrap();
    assert!(backend.get_fact(&fact_id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_update_fact() {
    let (_tmp, backend) = create_test_backend().await;
    let mut fact = make_test_fact("Original content", FactType::Preference, false);
    let fact_id = fact.id.clone();

    backend.insert_fact(&fact).await.unwrap();

    fact.content = "Updated content".to_string();
    backend.update_fact(&fact).await.unwrap();

    let retrieved = backend.get_fact(&fact_id).await.unwrap().unwrap();
    assert_eq!(retrieved.content, "Updated content");
}

#[tokio::test]
async fn test_batch_insert() {
    let (_tmp, backend) = create_test_backend().await;
    let facts = vec![
        make_test_fact("Fact A", FactType::Learning, false),
        make_test_fact("Fact B", FactType::Preference, false),
        make_test_fact("Fact C", FactType::Project, false),
    ];

    backend.batch_insert_facts(&facts).await.unwrap();

    for fact in &facts {
        let retrieved = backend.get_fact(&fact.id).await.unwrap();
        assert!(retrieved.is_some());
    }
}

#[tokio::test]
async fn test_batch_insert_empty() {
    let (_tmp, backend) = create_test_backend().await;
    backend.batch_insert_facts(&[]).await.unwrap();
}

#[tokio::test]
async fn test_invalidate_fact() {
    let (_tmp, backend) = create_test_backend().await;
    let fact = make_test_fact("Valid fact", FactType::Learning, false);
    let fact_id = fact.id.clone();

    backend.insert_fact(&fact).await.unwrap();
    backend
        .invalidate_fact(&fact_id, "superseded")
        .await
        .unwrap();

    let retrieved = backend.get_fact(&fact_id).await.unwrap().unwrap();
    assert!(!retrieved.is_valid);
    assert_eq!(
        retrieved.invalidation_reason,
        Some("superseded".to_string())
    );
}

#[tokio::test]
async fn test_update_fact_content() {
    let (_tmp, backend) = create_test_backend().await;
    let fact = make_test_fact("Old content", FactType::Personal, false);
    let fact_id = fact.id.clone();

    backend.insert_fact(&fact).await.unwrap();
    backend
        .update_fact_content(&fact_id, "New content")
        .await
        .unwrap();

    let retrieved = backend.get_fact(&fact_id).await.unwrap().unwrap();
    assert_eq!(retrieved.content, "New content");
}

#[tokio::test]
async fn test_get_all_facts() {
    let (_tmp, backend) = create_test_backend().await;
    let fact_valid = make_test_fact("Valid", FactType::Learning, false);
    let mut fact_invalid = make_test_fact("Invalid", FactType::Other, false);
    fact_invalid.is_valid = false;
    fact_invalid.invalidation_reason = Some("old".to_string());

    backend.insert_fact(&fact_valid).await.unwrap();
    backend.insert_fact(&fact_invalid).await.unwrap();

    // Without invalid
    let valid_only = backend.get_all_facts(false, None).await.unwrap();
    assert_eq!(valid_only.len(), 1);
    assert_eq!(valid_only[0].content, "Valid");

    // With invalid
    let all = backend.get_all_facts(true, None).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_count_facts() {
    let (_tmp, backend) = create_test_backend().await;
    let fact1 = make_test_fact("Fact 1", FactType::Learning, false);
    let fact2 = make_test_fact("Fact 2", FactType::Preference, false);
    let fact3 = make_test_fact("Fact 3", FactType::Learning, false);

    backend
        .batch_insert_facts(&[fact1, fact2, fact3])
        .await
        .unwrap();

    let total = backend.count_facts(&SearchFilter::new()).await.unwrap();
    assert_eq!(total, 3);

    let learning_only = backend
        .count_facts(&SearchFilter::new().with_fact_type(FactType::Learning))
        .await
        .unwrap();
    assert_eq!(learning_only, 2);
}

#[tokio::test]
async fn test_get_facts_by_type() {
    let (_tmp, backend) = create_test_backend().await;
    let fact1 = make_test_fact("Learning 1", FactType::Learning, false);
    let fact2 = make_test_fact("Preference 1", FactType::Preference, false);
    let fact3 = make_test_fact("Learning 2", FactType::Learning, false);

    backend
        .batch_insert_facts(&[fact1, fact2, fact3])
        .await
        .unwrap();

    let learning = backend
        .get_facts_by_type(FactType::Learning, &NamespaceScope::Owner, "main", 10)
        .await
        .unwrap();
    assert_eq!(learning.len(), 2);

    let prefs = backend
        .get_facts_by_type(FactType::Preference, &NamespaceScope::Owner, "main", 10)
        .await
        .unwrap();
    assert_eq!(prefs.len(), 1);
}

#[tokio::test]
async fn test_vector_search() {
    let (_tmp, backend) = create_test_backend().await;

    // Insert facts WITH embeddings
    let fact1 = make_test_fact("Rust programming", FactType::Learning, true);
    // fact1 has embedding = [0.1; 1024]

    let mut fact2 = make_test_fact("Python scripting", FactType::Learning, true);
    fact2.embedding = Some(vec![0.9_f32; 1024]);

    backend
        .batch_insert_facts(&[fact1.clone(), fact2.clone()])
        .await
        .unwrap();

    // Search with a vector close to fact1's embedding
    let query_vec = vec![0.1_f32; 1024];
    let results = backend
        .vector_search(&query_vec, 1024, &SearchFilter::new(), 10)
        .await
        .unwrap();

    assert!(!results.is_empty());
    // The result closest to [0.1; 1024] should be fact1
    assert_eq!(results[0].fact.content, "Rust programming");
    assert!(results[0].score > 0.0);
}

#[tokio::test]
async fn test_find_similar_facts() {
    let (_tmp, backend) = create_test_backend().await;

    let fact1 = make_test_fact("Similar fact", FactType::Learning, true);
    let mut fact2 = make_test_fact("Different fact", FactType::Learning, true);
    fact2.embedding = Some(vec![0.9_f32; 1024]);

    backend
        .batch_insert_facts(&[fact1.clone(), fact2.clone()])
        .await
        .unwrap();

    let query_vec = vec![0.1_f32; 1024];
    let results = backend
        .find_similar_facts(&query_vec, 1024, &SearchFilter::new(), 0.5, 10)
        .await
        .unwrap();

    // At least the very similar fact should be returned
    assert!(!results.is_empty());
    // All returned facts should meet the threshold
    for sf in &results {
        assert!(sf.score >= 0.5);
    }
}

#[tokio::test]
async fn test_list_by_path() {
    let (_tmp, backend) = create_test_backend().await;

    let mut fact1 = make_test_fact("Pref 1", FactType::Preference, false);
    fact1.path = "aleph://user/preferences/coding/".to_string();
    fact1.parent_path = "aleph://user/preferences/".to_string();

    let mut fact2 = make_test_fact("Pref 2", FactType::Preference, false);
    fact2.path = "aleph://user/preferences/ui/".to_string();
    fact2.parent_path = "aleph://user/preferences/".to_string();

    let mut fact3 = make_test_fact("Plan", FactType::Plan, false);
    fact3.path = "aleph://user/plans/trip/".to_string();
    fact3.parent_path = "aleph://user/plans/".to_string();

    backend
        .batch_insert_facts(&[fact1, fact2, fact3])
        .await
        .unwrap();

    let entries = backend
        .list_by_path("aleph://user/preferences/", &NamespaceScope::Owner, "main")
        .await
        .unwrap();

    assert_eq!(entries.len(), 2);
    let paths: HashSet<String> = entries.iter().map(|e| e.path.clone()).collect();
    assert!(paths.contains("aleph://user/preferences/coding/"));
    assert!(paths.contains("aleph://user/preferences/ui/"));
}

#[tokio::test]
async fn test_get_by_path() {
    let (_tmp, backend) = create_test_backend().await;

    let mut fact = make_test_fact("Coding preference", FactType::Preference, false);
    fact.path = "aleph://user/preferences/coding/rust".to_string();
    fact.parent_path = "aleph://user/preferences/coding/".to_string();

    backend.insert_fact(&fact).await.unwrap();

    let result = backend
        .get_by_path(
            "aleph://user/preferences/coding/rust",
            &NamespaceScope::Owner,
            "main",
        )
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().content, "Coding preference");

    // Non-existent path
    let missing = backend
        .get_by_path("aleph://nonexistent/path", &NamespaceScope::Owner, "main")
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn test_get_facts_by_path_prefix() {
    let (_tmp, backend) = create_test_backend().await;

    let mut fact_a = make_test_fact("A", FactType::Preference, false);
    fact_a.path = "aleph://user/preferences/coding/rust".to_string();
    fact_a.parent_path = "aleph://user/preferences/coding/".to_string();

    let mut fact_b = make_test_fact("B", FactType::Preference, false);
    fact_b.path = "aleph://user/preferences/coding/vim".to_string();
    fact_b.parent_path = "aleph://user/preferences/coding/".to_string();

    let mut fact_c = make_test_fact("C", FactType::Preference, false);
    fact_c.path = "aleph://user/preferences/ui/theme".to_string();
    fact_c.parent_path = "aleph://user/preferences/ui/".to_string();

    backend
        .batch_insert_facts(&[fact_a, fact_b, fact_c])
        .await
        .unwrap();

    let results = backend
        .get_facts_by_path_prefix(
            "aleph://user/preferences/coding/",
            &SearchFilter::new().with_fact_type(FactType::Preference),
            10,
        )
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|f| f.path.starts_with("aleph://user/preferences/coding/")));
}

#[tokio::test]
async fn test_invalidate_nonexistent_fact() {
    let (_tmp, backend) = create_test_backend().await;
    let result = backend
        .invalidate_fact("nonexistent", "reason")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_content_nonexistent_fact() {
    let (_tmp, backend) = create_test_backend().await;
    let result = backend
        .update_fact_content("nonexistent", "new content")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_search_with_filter() {
    let (_tmp, backend) = create_test_backend().await;

    let fact1 = make_test_fact("Learning Rust", FactType::Learning, true);
    let fact2 = make_test_fact("Preference coding", FactType::Preference, true);

    backend
        .batch_insert_facts(&[fact1, fact2])
        .await
        .unwrap();

    // Search with filter for Learning only
    let query_vec = vec![0.1_f32; 1024];
    let results = backend
        .vector_search(
            &query_vec,
            1024,
            &SearchFilter::new().with_fact_type(FactType::Learning),
            10,
        )
        .await
        .unwrap();

    // Only the Learning fact should be returned
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].fact.fact_type, FactType::Learning);
}
