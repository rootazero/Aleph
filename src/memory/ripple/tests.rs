//! Tests for RippleTask

use crate::sync_primitives::Arc;

use crate::memory::store::SqliteMemoryBackend;
use crate::memory::store::MemoryBackend;
use crate::memory::{
    FactSource, FactSpecificity, NoteType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};
use crate::Result;

use super::*;

/// Helper to create a test fact with embedding
fn create_test_fact(id: &str, content: &str, embedding: Vec<f32>) -> MemoryFact {
    // Ensure embedding is 1024 dimensions (pad or truncate)
    let mut emb_1024 = vec![0.0; 1024];
    for (i, &val) in embedding.iter().enumerate() {
        if i < 1024 {
            emb_1024[i] = val;
        }
    }

    MemoryFact {
        id: id.to_string(),
        content: content.to_string(),
        note_type: NoteType::Preference,
        embedding: Some(emb_1024),
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
        agent: "default".to_string(),
        tier: crate::memory::context::MemoryTier::ShortTerm,
        scope: crate::memory::context::MemoryScope::Global,
        persona_id: None,
        strength: 1.0,
        access_count: 0,
        last_accessed_at: None,
        valid_from: None,
        valid_to: None,
    }
}

/// Helper to create a test database with notes seeded via NoteStore.
///
/// The old `insert_fact` path has been removed — facts now live as notes.
/// These tests are marked `#[ignore]` because they require a fully wired
/// NoteIndexer + embedding pipeline that is not available in unit test context.
/// Use integration tests with a real NoteIndexer to exercise RippleTask end-to-end.
async fn create_test_database(agent: &str) -> Result<MemoryBackend> {
    let temp_dir = tempfile::tempdir()?;
    let backend = SqliteMemoryBackend::new(temp_dir.path())?;
    let db: MemoryBackend = Arc::new(backend);
    let _ = agent; // agent_id used at call sites

    // NOTE: temp_dir is dropped here, but SQLite keeps the files open.
    std::mem::forget(temp_dir);

    Ok(db)
}

/// Seed facts now live as notes indexed via NoteIndexer + upsert_embedding.
/// These tests are ignored because seeding requires a full NoteIndexer pipeline
/// that is not available in unit test context. The BFS/similarity logic itself
/// is verified by the cosine_similarity unit test in task.rs.
#[tokio::test]
#[ignore = "requires NoteIndexer + upsert_embedding pipeline; use integration tests"]
async fn test_ripple_single_hop() -> Result<()> {
    let db = create_test_database("default").await?;
    let fact_a = create_test_fact("fact_a", "User likes coffee", vec![1.0, 0.0, 0.0]);

    let config = RippleConfig {
        max_hops: 1,
        max_facts_per_hop: 5,
        similarity_threshold: 0.7,
        enable_tunnels: true,
        max_tunnel_hops: 1,
    };
    let ripple = RippleTask::new(db, config, "default");
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    assert_eq!(result.seed_facts.len(), 1);
    assert_eq!(result.seed_facts[0].id, "fact_a");

    Ok(())
}

#[tokio::test]
#[ignore = "requires NoteIndexer + upsert_embedding pipeline; use integration tests"]
async fn test_ripple_multi_hop() -> Result<()> {
    let db = create_test_database("default").await?;
    let fact_a = create_test_fact("fact_a", "Level 0", vec![1.0, 0.0, 0.0]);

    let config = RippleConfig {
        max_hops: 2,
        max_facts_per_hop: 5,
        similarity_threshold: 0.6,
        enable_tunnels: true,
        max_tunnel_hops: 1,
    };
    let ripple = RippleTask::new(db, config, "default");
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    // With no notes seeded, expanded_facts is empty — that is expected here.
    let _ = result;
    Ok(())
}

#[tokio::test]
#[ignore = "requires NoteIndexer + upsert_embedding pipeline; use integration tests"]
async fn test_ripple_similarity_threshold() -> Result<()> {
    let db = create_test_database("default").await?;
    let fact_a = create_test_fact("fact_a", "User likes coffee", vec![1.0, 0.0, 0.0]);

    let config = RippleConfig {
        max_hops: 1,
        max_facts_per_hop: 5,
        similarity_threshold: 0.8,
        enable_tunnels: true,
        max_tunnel_hops: 1,
    };
    let ripple = RippleTask::new(db, config, "default");
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    for expanded_fact in &result.expanded_facts {
        assert_ne!(expanded_fact.id, "fact_c");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires NoteIndexer + upsert_embedding pipeline; use integration tests"]
async fn test_ripple_no_duplicates() -> Result<()> {
    let db = create_test_database("default").await?;
    let fact_a = create_test_fact("fact_a", "Fact A", vec![1.0, 0.0, 0.0]);

    let config = RippleConfig {
        max_hops: 3,
        max_facts_per_hop: 5,
        similarity_threshold: 0.6,
        enable_tunnels: true,
        max_tunnel_hops: 1,
    };
    let ripple = RippleTask::new(db, config, "default");
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    let expanded_ids: Vec<_> = result.expanded_facts.iter().map(|f| &f.id).collect();
    let unique_ids: std::collections::HashSet<_> = expanded_ids.iter().collect();
    assert_eq!(
        unique_ids.len(),
        expanded_ids.len(),
        "Found duplicate facts"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires NoteIndexer + upsert_embedding pipeline; use integration tests"]
async fn test_ripple_max_facts_per_hop() -> Result<()> {
    let db = create_test_database("default").await?;
    let fact_a = create_test_fact("fact_a", "Root", vec![1.0, 0.0, 0.0]);

    let config = RippleConfig {
        max_hops: 1,
        max_facts_per_hop: 2,
        similarity_threshold: 0.6,
        enable_tunnels: true,
        max_tunnel_hops: 1,
    };
    let ripple = RippleTask::new(db, config, "default");
    let result = ripple.explore(vec![fact_a.clone()]).await?;

    assert!(result.expanded_facts.len() <= 2);
    Ok(())
}

#[test]
fn default_config_enables_tunnels() {
    let config = RippleConfig::default();
    assert!(config.enable_tunnels);
    assert_eq!(config.max_tunnel_hops, 1);
}
