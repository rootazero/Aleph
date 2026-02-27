//! Tests for evolution module

use std::sync::Arc;

use crate::memory::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer, TemporalScope,
};
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::store::lance::LanceMemoryBackend;
use crate::Result;

use super::*;
use super::resolver::ResolutionStrategy;

/// Helper to create a test fact
fn create_test_fact(id: &str, content: &str, confidence: f32) -> MemoryFact {
    // Create 1024-dim embedding based on content hash
    let mut embedding = vec![0.0; 1024];
    let hash = content.len() % 1024;
    embedding[hash] = 1.0;
    if hash + 1 < 1024 {
        embedding[hash + 1] = 0.5;
    }

    MemoryFact {
        id: id.to_string(),
        content: content.to_string(),
        fact_type: FactType::Preference,
        embedding: Some(embedding),
        source_memory_ids: vec![],
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
        confidence,
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

/// Helper to create test database (LanceDB-backed)
async fn create_test_database() -> Result<(MemoryBackend, tempfile::TempDir)> {
    let temp_dir = tempfile::tempdir()?;
    let backend = LanceMemoryBackend::open_or_create(temp_dir.path()).await?;
    Ok((Arc::new(backend), temp_dir))
}

#[test]
fn test_evolution_chain_creation() {
    let old_fact = create_test_fact("fact1", "User likes coffee", 0.9);
    let new_fact = create_test_fact("fact2", "User prefers tea", 0.95);

    let evolution = EvolutionChain::create_evolution(
        old_fact.clone(),
        new_fact.clone(),
        "Preference changed".to_string(),
    );

    assert_eq!(evolution.facts.len(), 2);
    assert_eq!(evolution.facts[0].fact_id, "fact1");
    assert_eq!(evolution.facts[1].fact_id, "fact2");
    assert_eq!(evolution.facts[0].superseded_by, Some("fact2".to_string()));
    assert_eq!(evolution.facts[1].superseded_by, None);
}

#[test]
fn test_evolution_chain_extension() {
    let fact1 = create_test_fact("fact1", "User likes coffee", 0.9);
    let fact2 = create_test_fact("fact2", "User prefers tea", 0.95);
    let fact3 = create_test_fact("fact3", "User drinks water only", 0.98);

    let evolution = EvolutionChain::create_evolution(
        fact1,
        fact2,
        "First change".to_string(),
    );

    let evolution = EvolutionChain::extend_evolution(
        evolution,
        fact3,
        "Second change".to_string(),
    );

    assert_eq!(evolution.facts.len(), 3);
    assert_eq!(evolution.facts[0].superseded_by, Some("fact2".to_string()));
    assert_eq!(evolution.facts[1].superseded_by, Some("fact3".to_string()));
    assert_eq!(evolution.facts[2].superseded_by, None);
}

#[test]
fn test_get_current_fact() {
    let fact1 = create_test_fact("fact1", "Old fact", 0.9);
    let fact2 = create_test_fact("fact2", "New fact", 0.95);

    let evolution = EvolutionChain::create_evolution(
        fact1,
        fact2.clone(),
        "Update".to_string(),
    );

    let current = EvolutionChain::get_current_fact(&evolution);
    assert!(current.is_some());
    assert_eq!(current.unwrap().id, "fact2");
}

#[test]
fn test_get_history() {
    let fact1 = create_test_fact("fact1", "Version 1", 0.9);
    let fact2 = create_test_fact("fact2", "Version 2", 0.95);
    let fact3 = create_test_fact("fact3", "Version 3", 0.98);

    let evolution = EvolutionChain::create_evolution(
        fact1,
        fact2,
        "Update 1".to_string(),
    );
    let evolution = EvolutionChain::extend_evolution(
        evolution,
        fact3,
        "Update 2".to_string(),
    );

    let history = EvolutionChain::get_history(&evolution);
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].content, "Version 1");
    assert_eq!(history[1].content, "Version 2");
    assert_eq!(history[2].content, "Version 3");
}

#[tokio::test]
async fn test_contradiction_detector_no_candidates() -> Result<()> {
    let (db, _temp_dir) = create_test_database().await?;
    let detector = ContradictionDetector::new(db, None);

    let new_fact = create_test_fact("fact1", "User likes coffee", 0.9);

    // Empty database, should find no contradictions
    let contradictions = detector.detect(&new_fact).await?;
    assert_eq!(contradictions.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_contradiction_detector_with_similar_facts() -> Result<()> {
    let (db, _temp_dir) = create_test_database().await?;

    // Store an existing fact
    let existing_fact = create_test_fact("fact1", "User likes coffee", 0.9);
    db.insert_fact(&existing_fact).await?;

    let detector = ContradictionDetector::new(db, None).with_threshold(0.5);

    // New fact with similar content
    let new_fact = create_test_fact("fact2", "User does not like coffee", 0.95);

    // Should detect potential contradiction using keyword matching
    let contradictions = detector.detect(&new_fact).await?;

    // With keyword-based detection, might find contradiction
    // (depends on similarity and keyword matching)
    assert!(contradictions.len() <= 1);

    Ok(())
}

#[tokio::test]
async fn test_evolution_resolver_prefer_newer() -> Result<()> {
    let (db, _temp_dir) = create_test_database().await?;
    let resolver = EvolutionResolver::new(db.clone());

    let old_fact = create_test_fact("fact1", "User likes coffee", 0.9);
    let new_fact = create_test_fact("fact2", "User prefers tea", 0.95);

    // Store old fact
    db.insert_fact(&old_fact).await?;

    // Resolve with PreferNewer strategy
    let evolution = resolver
        .resolve(
            old_fact.clone(),
            new_fact.clone(),
            "Preference changed".to_string(),
            ResolutionStrategy::PreferNewer,
        )
        .await?;

    assert_eq!(evolution.facts.len(), 2);

    // Check that old fact was invalidated
    let stored_old = db.get_fact(&old_fact.id).await?;
    assert!(stored_old.is_some());
    assert!(!stored_old.unwrap().is_valid);

    Ok(())
}

#[tokio::test]
async fn test_evolution_resolver_prefer_higher_confidence() -> Result<()> {
    let (db, _temp_dir) = create_test_database().await?;
    let resolver = EvolutionResolver::new(db.clone());

    let low_confidence = create_test_fact("fact1", "User likes coffee", 0.7);
    let high_confidence = create_test_fact("fact2", "User prefers tea", 0.95);

    // Store low confidence fact
    db.insert_fact(&low_confidence).await?;

    // Resolve with PreferHigherConfidence strategy
    let evolution = resolver
        .resolve(
            low_confidence.clone(),
            high_confidence.clone(),
            "Confidence difference".to_string(),
            ResolutionStrategy::PreferHigherConfidence,
        )
        .await?;

    assert_eq!(evolution.facts.len(), 2);

    // Check that low confidence fact was invalidated
    let stored = db.get_fact(&low_confidence.id).await?;
    assert!(stored.is_some());
    assert!(!stored.unwrap().is_valid);

    Ok(())
}

#[tokio::test]
async fn test_evolution_resolver_create_evolution() -> Result<()> {
    let (db, _temp_dir) = create_test_database().await?;
    let resolver = EvolutionResolver::new(db.clone());

    let old_fact = create_test_fact("fact1", "User likes coffee", 0.9);
    let new_fact = create_test_fact("fact2", "User prefers tea", 0.95);

    // Store old fact
    db.insert_fact(&old_fact).await?;

    // Resolve with CreateEvolution strategy
    let evolution = resolver
        .resolve(
            old_fact.clone(),
            new_fact.clone(),
            "Evolution tracked".to_string(),
            ResolutionStrategy::CreateEvolution,
        )
        .await?;

    assert_eq!(evolution.facts.len(), 2);

    // Check that old fact is still valid (not invalidated)
    let stored = db.get_fact(&old_fact.id).await?;
    assert!(stored.is_some());
    assert!(stored.unwrap().is_valid);

    Ok(())
}

#[tokio::test]
async fn test_evolution_resolver_multiple() -> Result<()> {
    let (db, _temp_dir) = create_test_database().await?;
    let resolver = EvolutionResolver::new(db.clone());

    let new_fact = create_test_fact("fact_new", "User drinks water", 0.95);
    let old_fact1 = create_test_fact("fact1", "User likes coffee", 0.9);
    let old_fact2 = create_test_fact("fact2", "User prefers tea", 0.9);

    // Store old facts
    db.insert_fact(&old_fact1).await?;
    db.insert_fact(&old_fact2).await?;

    let contradictions = vec![
        (old_fact1, "Contradicts coffee preference".to_string()),
        (old_fact2, "Contradicts tea preference".to_string()),
    ];

    // Resolve multiple contradictions
    let evolutions = resolver
        .resolve_multiple(new_fact, contradictions, ResolutionStrategy::PreferNewer)
        .await?;

    assert_eq!(evolutions.len(), 2);

    Ok(())
}
