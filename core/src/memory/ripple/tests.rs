//! Tests for RippleTask

use std::sync::Arc;

use crate::memory::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::store::lance::LanceMemoryBackend;
use crate::Result;

use super::*;

/// Helper to create a test fact with embedding
fn create_test_fact(id: &str, content: &str, embedding: Vec<f32>) -> MemoryFact {
    // Ensure embedding is 384 dimensions (pad or truncate)
    let mut emb_384 = vec![0.0; 384];
    for (i, &val) in embedding.iter().enumerate() {
        if i < 384 {
            emb_384[i] = val;
        }
    }

    MemoryFact {
        id: id.to_string(),
        content: content.to_string(),
        fact_type: FactType::Preference,
        embedding: Some(emb_384),
        source_memory_ids: vec![],
        created_at: 0,
        updated_at: 0,
        confidence: 0.9,
        is_valid: true,
        invalidation_reason: None,
        decay_invalidated_at: None,
        specificity: FactSpecificity::Pattern,
        temporal_scope: TemporalScope::Permanent,
        similarity_score: None,
        path: String::new(),
        layer: MemoryLayer::L2Detail,
        category: MemoryCategory::Preferences,
        fact_source: FactSource::Extracted,
        content_hash: String::new(),
        parent_path: String::new(),
        embedding_model: String::new(),
        namespace: "owner".to_string(),
        workspace: "default".to_string(),
        tier: crate::memory::context::MemoryTier::ShortTerm,
        scope: crate::memory::context::MemoryScope::Global,
        persona_id: None,
        strength: 1.0,
        access_count: 0,
        last_accessed_at: None,
    }
}

/// Helper to create a test database with facts (LanceDB-backed)
async fn create_test_database_with_facts(facts: Vec<MemoryFact>) -> Result<MemoryBackend> {
    let temp_dir = tempfile::tempdir()?;
    let backend = LanceMemoryBackend::open_or_create(temp_dir.path()).await?;
    let db: MemoryBackend = Arc::new(backend);

    // Store all facts
    for fact in &facts {
        db.insert_fact(fact).await?;
    }

    // NOTE: temp_dir is dropped here, but LanceDB keeps the files open.
    // For proper test isolation, we leak the tempdir to prevent premature cleanup.
    std::mem::forget(temp_dir);

    Ok(db)
}

#[tokio::test]
async fn test_ripple_single_hop() -> Result<()> {
    // Create facts with similar embeddings
    let fact_a = create_test_fact("fact_a", "User likes coffee", vec![1.0, 0.0, 0.0]);
    let fact_b = create_test_fact("fact_b", "User drinks espresso", vec![0.9, 0.1, 0.0]);
    let fact_c = create_test_fact("fact_c", "User prefers morning coffee", vec![0.8, 0.2, 0.0]);

    // Create database with facts
    let db = create_test_database_with_facts(vec![
        fact_a.clone(),
        fact_b.clone(),
        fact_c.clone(),
    ])
    .await?;

    // Create RippleTask with max_hops=1
    let config = RippleConfig {
        max_hops: 1,
        max_facts_per_hop: 5,
        similarity_threshold: 0.7,
    };
    let ripple = RippleTask::new(db, config);

    // Explore from fact_a
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    // Verify results
    assert_eq!(result.seed_facts.len(), 1);
    assert_eq!(result.seed_facts[0].id, "fact_a");

    // Should find fact_b and fact_c (both similar enough)
    assert!(result.expanded_facts.len() >= 1);

    Ok(())
}

#[tokio::test]
async fn test_ripple_multi_hop() -> Result<()> {
    // Create facts forming a similarity chain
    let fact_a = create_test_fact("fact_a", "Level 0", vec![1.0, 0.0, 0.0]);
    let fact_b = create_test_fact("fact_b", "Level 1", vec![0.9, 0.1, 0.0]);
    let fact_c = create_test_fact("fact_c", "Level 2", vec![0.8, 0.2, 0.0]);
    let fact_d = create_test_fact("fact_d", "Level 3", vec![0.7, 0.3, 0.0]);

    // Create database with facts
    let db = create_test_database_with_facts(vec![
        fact_a.clone(),
        fact_b.clone(),
        fact_c.clone(),
        fact_d.clone(),
    ])
    .await?;

    // Create RippleTask with max_hops=2
    let config = RippleConfig {
        max_hops: 2,
        max_facts_per_hop: 5,
        similarity_threshold: 0.6,
    };
    let ripple = RippleTask::new(db, config);

    // Explore from fact_a
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    // Should find multiple facts through multi-hop exploration
    assert!(result.expanded_facts.len() >= 1);

    Ok(())
}

#[tokio::test]
async fn test_ripple_similarity_threshold() -> Result<()> {
    // Create facts: B is similar, C is dissimilar
    let fact_a = create_test_fact("fact_a", "User likes coffee", vec![1.0, 0.0, 0.0]);
    let fact_b = create_test_fact("fact_b", "Similar fact", vec![0.9, 0.1, 0.0]);
    let fact_c = create_test_fact("fact_c", "Dissimilar fact", vec![0.0, 0.0, 1.0]);

    // Create database with facts
    let db = create_test_database_with_facts(vec![
        fact_a.clone(),
        fact_b.clone(),
        fact_c.clone(),
    ])
    .await?;

    // Create RippleTask with high similarity threshold
    let config = RippleConfig {
        max_hops: 1,
        max_facts_per_hop: 5,
        similarity_threshold: 0.8,
    };
    let ripple = RippleTask::new(db, config);

    // Explore from fact_a
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    // Should only find similar facts
    for expanded_fact in &result.expanded_facts {
        assert_ne!(expanded_fact.id, "fact_c"); // Dissimilar fact should not be included
    }

    Ok(())
}

#[tokio::test]
async fn test_ripple_no_duplicates() -> Result<()> {
    // Create facts with circular similarity
    let fact_a = create_test_fact("fact_a", "Fact A", vec![1.0, 0.0, 0.0]);
    let fact_b = create_test_fact("fact_b", "Fact B", vec![0.9, 0.1, 0.0]);
    let fact_c = create_test_fact("fact_c", "Fact C", vec![0.8, 0.2, 0.0]);

    // Create database with facts
    let db = create_test_database_with_facts(vec![
        fact_a.clone(),
        fact_b.clone(),
        fact_c.clone(),
    ])
    .await?;

    // Create RippleTask
    let config = RippleConfig {
        max_hops: 3,
        max_facts_per_hop: 5,
        similarity_threshold: 0.6,
    };
    let ripple = RippleTask::new(db, config);

    // Explore from fact_a
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    // Verify no duplicates
    let expanded_ids: Vec<_> = result.expanded_facts.iter().map(|f| &f.id).collect();
    let unique_ids: std::collections::HashSet<_> = expanded_ids.iter().collect();
    assert_eq!(unique_ids.len(), expanded_ids.len(), "Found duplicate facts");

    Ok(())
}

#[tokio::test]
async fn test_ripple_max_facts_per_hop() -> Result<()> {
    // Create facts with varying similarity
    let fact_a = create_test_fact("fact_a", "Root", vec![1.0, 0.0, 0.0]);
    let fact_b1 = create_test_fact("fact_b1", "Child 1", vec![0.9, 0.1, 0.0]);
    let fact_b2 = create_test_fact("fact_b2", "Child 2", vec![0.9, 0.0, 0.1]);
    let fact_b3 = create_test_fact("fact_b3", "Child 3", vec![0.8, 0.1, 0.1]);

    // Create database with facts
    let db = create_test_database_with_facts(vec![
        fact_a.clone(),
        fact_b1.clone(),
        fact_b2.clone(),
        fact_b3.clone(),
    ])
    .await?;

    // Create RippleTask with max_facts_per_hop=2
    let config = RippleConfig {
        max_hops: 1,
        max_facts_per_hop: 2,
        similarity_threshold: 0.6,
    };
    let ripple = RippleTask::new(db, config);

    // Explore from fact_a
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    // Should find at most 2 facts per hop
    assert!(result.expanded_facts.len() <= 2);

    Ok(())
}
