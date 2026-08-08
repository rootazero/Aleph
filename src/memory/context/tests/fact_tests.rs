use crate::memory::context::*;

#[test]
fn test_fact_specificity() {
    let fact = MemoryFact::new(
        "User prefers Rust".to_string(),
        NoteType::Preference,
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
        NoteType::Preference,
        vec![],
    );
    // Default should be Pattern and Contextual
    assert_eq!(fact.specificity, FactSpecificity::Pattern);
    assert_eq!(fact.temporal_scope, TemporalScope::Contextual);
}

#[test]
fn test_memory_fact_defaults_layer_and_category() {
    let fact = MemoryFact::new("User likes Vim".to_string(), NoteType::Preference, vec![]);
    assert_eq!(fact.layer, MemoryLayer::L2Detail);
    assert_eq!(fact.category, MemoryCategory::Preferences);
}

#[test]
fn test_memory_fact_new_has_path_fields() {
    let fact = MemoryFact::new(
        "User prefers Rust".to_string(),
        NoteType::Preference,
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
        NoteType::Learning,
        vec![],
    )
    .with_path("aleph://knowledge/learning/wasm/".to_string());
    assert_eq!(fact.path, "aleph://knowledge/learning/wasm/");
    assert_eq!(fact.parent_path, "aleph://knowledge/learning/");
}

#[test]
fn test_compute_parent_path() {
    assert_eq!(
        compute_parent_path("aleph://user/preferences/coding/"),
        "aleph://user/preferences/"
    );
    assert_eq!(
        compute_parent_path("aleph://user/preferences/"),
        "aleph://user/"
    );
    assert_eq!(compute_parent_path("aleph://user/"), "aleph://");
    assert_eq!(compute_parent_path(""), "");
}

#[test]
fn test_memory_fact_defaults() {
    let fact = MemoryFact::new("User likes Vim".to_string(), NoteType::Preference, vec![]);
    assert_eq!(fact.persona_id, None);
    assert_eq!(fact.access_count, 0);
    assert_eq!(fact.last_accessed_at, None);
}

#[test]
fn test_memory_fact_with_persona() {
    let fact = MemoryFact::new(
        "User prefers dark mode".to_string(),
        NoteType::Preference,
        vec![],
    )
    .with_persona_id("persona-coder".to_string());

    assert_eq!(fact.persona_id, Some("persona-coder".to_string()));
}
