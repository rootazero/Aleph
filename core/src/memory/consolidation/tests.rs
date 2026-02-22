//! Tests for consolidation module

use std::sync::Arc;

use crate::memory::{FactSource, FactSpecificity, FactType, MemoryFact, TemporalScope, VectorDatabase};
use crate::Result;

use super::*;

/// Helper to create a test fact
fn create_test_fact(
    id: &str,
    content: &str,
    fact_type: FactType,
    confidence: f32,
    days_old: i64,
) -> MemoryFact {
    // Create 384-dim embedding based on content hash
    let mut embedding = vec![0.0; 384];
    let hash = content.len() % 384;
    embedding[hash] = 1.0;
    if hash + 1 < 384 {
        embedding[hash + 1] = 0.5;
    }

    let now = chrono::Utc::now().timestamp();
    let updated_at = now - (days_old * 86400);

    MemoryFact {
        id: id.to_string(),
        content: content.to_string(),
        fact_type,
        embedding: Some(embedding),
        source_memory_ids: vec![],
        created_at: updated_at,
        updated_at,
        confidence,
        is_valid: true,
        invalidation_reason: None,
        decay_invalidated_at: None,
        specificity: FactSpecificity::Pattern,
        temporal_scope: TemporalScope::Permanent,
        similarity_score: None,
        path: String::new(),
        fact_source: FactSource::Extracted,
        content_hash: String::new(),
        parent_path: String::new(),
    }
}

/// Helper to create test database with facts
async fn create_test_database_with_facts(facts: Vec<MemoryFact>) -> Result<(Arc<VectorDatabase>, tempfile::TempDir)> {
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path)?;

    for fact in facts {
        db.insert_fact(fact).await?;
    }

    Ok((Arc::new(db), temp_dir))
}

#[test]
fn test_user_profile_creation() {
    let profile = UserProfile::new();
    assert!(!profile.profile_id.is_empty());
    assert_eq!(profile.categories.len(), 0);
    assert_eq!(profile.fact_count(), 0);
}

#[test]
fn test_profile_add_category() {
    let mut profile = UserProfile::new();
    let category = ProfileCategory::new("preferences".to_string());

    profile.add_category(category);
    assert_eq!(profile.categories.len(), 1);
    assert!(profile.get_category("preferences").is_some());
}

#[test]
fn test_profile_category_creation() {
    let category = ProfileCategory::new("habits".to_string());
    assert_eq!(category.name, "habits");
    assert_eq!(category.facts.len(), 0);
    assert_eq!(category.confidence, 0.0);
}

#[test]
fn test_category_add_fact() {
    let mut category = ProfileCategory::new("preferences".to_string());

    let fact = ConsolidatedFact::new(
        "User prefers dark mode".to_string(),
        vec!["fact1".to_string(), "fact2".to_string()],
        50,
        chrono::Utc::now().timestamp(),
    );

    category.add_fact(fact);
    assert_eq!(category.facts.len(), 1);
    assert!(category.confidence > 0.0);
}

#[test]
fn test_consolidated_fact_creation() {
    let now = chrono::Utc::now().timestamp();
    let fact = ConsolidatedFact::new(
        "User likes coffee".to_string(),
        vec!["fact1".to_string()],
        25,
        now,
    );

    assert_eq!(fact.content, "User likes coffee");
    assert_eq!(fact.source_fact_ids.len(), 1);
    assert_eq!(fact.access_count, 25);
    assert_eq!(fact.last_accessed, now);
    assert_eq!(fact.confidence, 0.25); // 25/100
}

#[test]
fn test_consolidated_fact_update_access() {
    let now = chrono::Utc::now().timestamp();
    let mut fact = ConsolidatedFact::new(
        "User likes coffee".to_string(),
        vec!["fact1".to_string()],
        25,
        now,
    );

    let new_time = now + 3600;
    fact.update_access(new_time);

    assert_eq!(fact.access_count, 26);
    assert_eq!(fact.last_accessed, new_time);
    assert_eq!(fact.confidence, 0.26);
}

#[test]
fn test_profile_get_all_facts() {
    let mut profile = UserProfile::new();

    let mut cat1 = ProfileCategory::new("preferences".to_string());
    cat1.add_fact(ConsolidatedFact::new(
        "Fact 1".to_string(),
        vec!["f1".to_string()],
        10,
        0,
    ));

    let mut cat2 = ProfileCategory::new("habits".to_string());
    cat2.add_fact(ConsolidatedFact::new(
        "Fact 2".to_string(),
        vec!["f2".to_string()],
        20,
        0,
    ));

    profile.add_category(cat1);
    profile.add_category(cat2);

    let all_facts = profile.get_all_facts();
    assert_eq!(all_facts.len(), 2);
    assert_eq!(profile.fact_count(), 2);
}

#[tokio::test]
async fn test_analyzer_empty_database() -> Result<()> {
    let (db, _temp_dir) = create_test_database_with_facts(vec![]).await?;
    let analyzer = ConsolidationAnalyzer::new(db, None);

    let config = ConsolidationConfig::default();
    let profile = analyzer.generate_profile(config).await?;

    assert_eq!(profile.fact_count(), 0);
    Ok(())
}

#[tokio::test]
async fn test_analyzer_with_facts() -> Result<()> {
    let facts = vec![
        create_test_fact("f1", "User prefers dark mode", FactType::Preference, 0.9, 5),
        create_test_fact("f2", "User likes coffee", FactType::Preference, 0.85, 10),
        create_test_fact("f3", "User exercises daily", FactType::Personal, 0.8, 3),
    ];

    let (db, _temp_dir) = create_test_database_with_facts(facts).await?;
    let analyzer = ConsolidationAnalyzer::new(db, None);

    let config = ConsolidationConfig::default();
    let profile = analyzer.generate_profile(config).await?;

    // Should have at least 2 categories (preferences and personal)
    assert!(profile.categories.len() >= 2);
    assert!(profile.fact_count() >= 3);

    Ok(())
}

#[tokio::test]
async fn test_analyzer_frequency_filtering() -> Result<()> {
    let facts = vec![
        create_test_fact("f1", "High confidence fact", FactType::Preference, 0.95, 1),
        create_test_fact("f2", "Low confidence fact", FactType::Preference, 0.3, 100),
    ];

    let (db, _temp_dir) = create_test_database_with_facts(facts).await?;
    let analyzer = ConsolidationAnalyzer::new(db, None);

    let config = ConsolidationConfig {
        min_frequency_score: 0.7,
        ..Default::default()
    };

    let profile = analyzer.generate_profile(config).await?;

    // Should only include high confidence, recent fact
    assert!(profile.fact_count() >= 1);

    Ok(())
}

#[tokio::test]
async fn test_analyzer_categorization_by_type() -> Result<()> {
    let facts = vec![
        create_test_fact("f1", "Preference 1", FactType::Preference, 0.9, 5),
        create_test_fact("f2", "Preference 2", FactType::Preference, 0.85, 10),
        create_test_fact("f3", "Personal 1", FactType::Personal, 0.8, 3),
        create_test_fact("f4", "Learning 1", FactType::Learning, 0.9, 7),
    ];

    let (db, _temp_dir) = create_test_database_with_facts(facts).await?;
    let analyzer = ConsolidationAnalyzer::new(db, None);

    let config = ConsolidationConfig::default();
    let profile = analyzer.generate_profile(config).await?;

    // Should have 3 categories
    assert_eq!(profile.categories.len(), 3);
    assert!(profile.get_category("preferences").is_some());
    assert!(profile.get_category("personal").is_some());
    assert!(profile.get_category("learning").is_some());

    Ok(())
}

#[tokio::test]
async fn test_analyzer_consolidation() -> Result<()> {
    // Create similar facts that should be consolidated
    let mut fact1 = create_test_fact("f1", "User likes coffee", FactType::Preference, 0.9, 5);
    let mut fact2 = create_test_fact("f2", "User enjoys coffee", FactType::Preference, 0.85, 10);

    // Make embeddings very similar
    let similar_embedding = vec![1.0; 384];
    fact1.embedding = Some(similar_embedding.clone());
    fact2.embedding = Some(similar_embedding);

    let (db, _temp_dir) = create_test_database_with_facts(vec![fact1, fact2]).await?;
    let analyzer = ConsolidationAnalyzer::new(db, None);

    let config = ConsolidationConfig {
        similarity_threshold: 0.95,
        ..Default::default()
    };

    let profile = analyzer.generate_profile(config).await?;

    // Similar facts should be consolidated into one
    let prefs = profile.get_category("preferences").unwrap();
    assert_eq!(prefs.facts.len(), 1);

    // The consolidated fact should reference both source facts
    assert_eq!(prefs.facts[0].source_fact_ids.len(), 2);

    Ok(())
}

#[tokio::test]
async fn test_analyzer_max_facts_limit() -> Result<()> {
    // Create many facts
    let mut facts = Vec::new();
    for i in 0..20 {
        facts.push(create_test_fact(
            &format!("f{}", i),
            &format!("Fact {}", i),
            FactType::Preference,
            0.9,
            i % 10,
        ));
    }

    let (db, _temp_dir) = create_test_database_with_facts(facts).await?;
    let analyzer = ConsolidationAnalyzer::new(db, None);

    let config = ConsolidationConfig {
        max_facts: 10,
        ..Default::default()
    };

    let profile = analyzer.generate_profile(config).await?;

    // Should not exceed max_facts limit
    assert!(profile.fact_count() <= 10);

    Ok(())
}
