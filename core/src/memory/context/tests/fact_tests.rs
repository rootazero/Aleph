use crate::memory::context::*;

#[test]
fn test_context_anchor_now() {
    let anchor = ContextAnchor::now("Test.txt".to_string());
    assert_eq!(anchor.window_title, "Test.txt");
    assert!(anchor.timestamp > 0);
}

#[test]
fn test_context_anchor_with_timestamp() {
    let anchor = ContextAnchor::with_timestamp("Test.txt".to_string(), 1234567890);
    assert_eq!(anchor.timestamp, 1234567890);
}

#[test]
fn test_memory_entry_new() {
    let context = ContextAnchor::now("window".to_string());
    let entry = MemoryEntry::new(
        "id-123".to_string(),
        context.clone(),
        "user input".to_string(),
        "ai output".to_string(),
    );
    assert_eq!(entry.id, "id-123");
    assert_eq!(entry.context, context);
    assert!(entry.embedding.is_none());
    assert!(entry.similarity_score.is_none());
}

#[test]
fn test_memory_entry_with_embedding() {
    let context = ContextAnchor::now("window".to_string());
    let embedding = vec![0.1, 0.2, 0.3];
    let entry = MemoryEntry::with_embedding(
        "id-123".to_string(),
        context,
        "input".to_string(),
        "output".to_string(),
        embedding.clone(),
    );
    assert_eq!(entry.embedding, Some(embedding));
}

#[test]
fn test_memory_entry_with_score() {
    let context = ContextAnchor::now("window".to_string());
    let entry = MemoryEntry::new(
        "id".to_string(),
        context,
        "in".to_string(),
        "out".to_string(),
    )
    .with_score(0.85);
    assert_eq!(entry.similarity_score, Some(0.85));
}

#[test]
fn test_context_anchor_serialization() {
    let anchor = ContextAnchor::with_timestamp("Test.txt".to_string(), 1234567890);
    let json = serde_json::to_string(&anchor).unwrap();
    let deserialized: ContextAnchor = serde_json::from_str(&json).unwrap();
    assert_eq!(anchor, deserialized);
}

#[test]
fn test_fact_specificity() {
    let fact = MemoryFact::new(
        "User prefers Rust".to_string(),
        FactType::Preference,
        vec!["mem-1".to_string()],
    )
    .with_specificity(FactSpecificity::Pattern)
    .with_temporal_scope(TemporalScope::Permanent);

    assert_eq!(fact.specificity, FactSpecificity::Pattern);
    assert_eq!(fact.temporal_scope, TemporalScope::Permanent);
}

#[test]
fn test_fact_specificity_default() {
    let fact = MemoryFact::new(
        "User likes coding".to_string(),
        FactType::Preference,
        vec![],
    );
    // Default should be Pattern and Contextual
    assert_eq!(fact.specificity, FactSpecificity::Pattern);
    assert_eq!(fact.temporal_scope, TemporalScope::Contextual);
}

#[test]
fn test_memory_fact_defaults_layer_and_category() {
    let fact = MemoryFact::new("User likes Vim".to_string(), FactType::Preference, vec![]);
    assert_eq!(fact.layer, MemoryLayer::L2Detail);
    assert_eq!(fact.category, MemoryCategory::Preferences);
}

#[test]
fn test_memory_fact_new_has_path_fields() {
    let fact = MemoryFact::new(
        "User prefers Rust".to_string(),
        FactType::Preference,
        vec!["src-1".to_string()],
    );
    assert_eq!(fact.path, "aleph://user/preferences/");
    assert_eq!(fact.parent_path, "aleph://user/");
    assert_eq!(fact.fact_source, FactSource::Extracted);
    assert!(fact.content_hash.is_empty());
}

#[test]
fn test_memory_fact_with_path() {
    let fact = MemoryFact::new(
        "Learning WebAssembly".to_string(),
        FactType::Learning,
        vec![],
    )
    .with_path("aleph://knowledge/learning/wasm/".to_string());
    assert_eq!(fact.path, "aleph://knowledge/learning/wasm/");
    assert_eq!(fact.parent_path, "aleph://knowledge/learning/");
}

#[test]
fn test_compute_parent_path() {
    assert_eq!(compute_parent_path("aleph://user/preferences/coding/"), "aleph://user/preferences/");
    assert_eq!(compute_parent_path("aleph://user/preferences/"), "aleph://user/");
    assert_eq!(compute_parent_path("aleph://user/"), "aleph://");
    assert_eq!(compute_parent_path(""), "");
}

#[test]
fn test_memory_fact_defaults_tier_and_scope() {
    let fact = MemoryFact::new("User likes Vim".to_string(), FactType::Preference, vec![]);
    assert_eq!(fact.tier, MemoryTier::ShortTerm);
    assert_eq!(fact.scope, MemoryScope::Global);
    assert_eq!(fact.persona_id, None);
    assert!((fact.strength - 1.0).abs() < f32::EPSILON);
    assert_eq!(fact.access_count, 0);
    assert_eq!(fact.last_accessed_at, None);
}

#[test]
fn test_memory_fact_with_persona() {
    let fact = MemoryFact::new(
        "User prefers dark mode".to_string(),
        FactType::Preference,
        vec![],
    )
    .with_tier(MemoryTier::Core)
    .with_scope(MemoryScope::Persona)
    .with_persona_id("persona-coder".to_string());

    assert_eq!(fact.tier, MemoryTier::Core);
    assert_eq!(fact.scope, MemoryScope::Persona);
    assert_eq!(fact.persona_id, Some("persona-coder".to_string()));
}
