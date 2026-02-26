use crate::memory::context::*;

#[test]
fn test_fact_specificity_from_str() {
    assert_eq!(
        FactSpecificity::from_str_or_default("principle"),
        FactSpecificity::Principle
    );
    assert_eq!(
        FactSpecificity::from_str_or_default("PATTERN"),
        FactSpecificity::Pattern
    );
    assert_eq!(
        FactSpecificity::from_str_or_default("instance"),
        FactSpecificity::Instance
    );
    assert_eq!(
        FactSpecificity::from_str_or_default("unknown"),
        FactSpecificity::Pattern
    ); // default
}

#[test]
fn test_temporal_scope_from_str() {
    assert_eq!(
        TemporalScope::from_str_or_default("permanent"),
        TemporalScope::Permanent
    );
    assert_eq!(
        TemporalScope::from_str_or_default("CONTEXTUAL"),
        TemporalScope::Contextual
    );
    assert_eq!(
        TemporalScope::from_str_or_default("ephemeral"),
        TemporalScope::Ephemeral
    );
    assert_eq!(
        TemporalScope::from_str_or_default("unknown"),
        TemporalScope::Contextual
    ); // default
}

#[test]
fn test_fact_specificity_as_str() {
    assert_eq!(FactSpecificity::Principle.as_str(), "principle");
    assert_eq!(FactSpecificity::Pattern.as_str(), "pattern");
    assert_eq!(FactSpecificity::Instance.as_str(), "instance");
}

#[test]
fn test_temporal_scope_as_str() {
    assert_eq!(TemporalScope::Permanent.as_str(), "permanent");
    assert_eq!(TemporalScope::Contextual.as_str(), "contextual");
    assert_eq!(TemporalScope::Ephemeral.as_str(), "ephemeral");
}

#[test]
fn test_fact_type_tool() {
    assert_eq!(FactType::Tool.as_str(), "tool");
    assert_eq!(FactType::from_str_or_other("tool"), FactType::Tool);
}

#[test]
fn test_subagent_fact_types() {
    assert_eq!(FactType::from_str_or_other("subagent_run"), FactType::SubagentRun);
    assert_eq!(FactType::from_str_or_other("subagent_session"), FactType::SubagentSession);
    assert_eq!(FactType::from_str_or_other("subagent_checkpoint"), FactType::SubagentCheckpoint);
    assert_eq!(FactType::from_str_or_other("subagent_transcript"), FactType::SubagentTranscript);
    assert_eq!(FactType::SubagentRun.as_str(), "subagent_run");
    assert_eq!(FactType::SubagentSession.as_str(), "subagent_session");
    assert_eq!(FactType::SubagentCheckpoint.as_str(), "subagent_checkpoint");
    assert_eq!(FactType::SubagentTranscript.as_str(), "subagent_transcript");
}

#[test]
fn test_fact_source_roundtrip() {
    assert_eq!(FactSource::Extracted.as_str(), "extracted");
    assert_eq!(FactSource::Summary.as_str(), "summary");
    assert_eq!(FactSource::Document.as_str(), "document");
    assert_eq!(FactSource::Manual.as_str(), "manual");
    assert_eq!(FactSource::from_str_or_default("summary"), FactSource::Summary);
    assert_eq!(FactSource::from_str_or_default("unknown"), FactSource::Extracted);
}

#[test]
fn test_memory_layer_roundtrip() {
    assert_eq!(MemoryLayer::L0Abstract.as_str(), "l0_abstract");
    assert_eq!(
        MemoryLayer::from_str_or_default("l1_overview"),
        MemoryLayer::L1Overview
    );
    assert_eq!(
        MemoryLayer::from_str_or_default("unknown"),
        MemoryLayer::L2Detail
    );
}

#[test]
fn test_memory_category_roundtrip() {
    assert_eq!(MemoryCategory::Profile.as_str(), "profile");
    assert_eq!(
        MemoryCategory::from_str_or_default("patterns"),
        MemoryCategory::Patterns
    );
    assert_eq!(
        MemoryCategory::from_str_or_default("unknown"),
        MemoryCategory::Entities
    );
}

#[test]
fn test_memory_tier_roundtrip() {
    assert_eq!(MemoryTier::Core.as_str(), "core");
    assert_eq!(MemoryTier::ShortTerm.as_str(), "short_term");
    assert_eq!(MemoryTier::LongTerm.as_str(), "long_term");
    assert_eq!(
        MemoryTier::from_str_or_default("core"),
        MemoryTier::Core
    );
    assert_eq!(
        MemoryTier::from_str_or_default("short_term"),
        MemoryTier::ShortTerm
    );
    assert_eq!(
        MemoryTier::from_str_or_default("long_term"),
        MemoryTier::LongTerm
    );
    assert_eq!(
        MemoryTier::from_str_or_default("unknown"),
        MemoryTier::ShortTerm
    ); // default
    assert_eq!(format!("{}", MemoryTier::Core), "core");
}

#[test]
fn test_memory_scope_roundtrip() {
    assert_eq!(MemoryScope::Global.as_str(), "global");
    assert_eq!(MemoryScope::Workspace.as_str(), "workspace");
    assert_eq!(MemoryScope::Persona.as_str(), "persona");
    assert_eq!(
        MemoryScope::from_str_or_default("global"),
        MemoryScope::Global
    );
    assert_eq!(
        MemoryScope::from_str_or_default("workspace"),
        MemoryScope::Workspace
    );
    assert_eq!(
        MemoryScope::from_str_or_default("persona"),
        MemoryScope::Persona
    );
    assert_eq!(
        MemoryScope::from_str_or_default("unknown"),
        MemoryScope::Global
    ); // default
    assert_eq!(format!("{}", MemoryScope::Persona), "persona");
}

#[test]
fn test_fact_type_default_path() {
    assert_eq!(FactType::Preference.default_path(), "aleph://user/preferences/");
    assert_eq!(FactType::Personal.default_path(), "aleph://user/personal/");
    assert_eq!(FactType::Plan.default_path(), "aleph://user/plans/");
    assert_eq!(FactType::Learning.default_path(), "aleph://knowledge/learning/");
    assert_eq!(FactType::Project.default_path(), "aleph://knowledge/projects/");
    assert_eq!(FactType::Tool.default_path(), "aleph://agent/tools/");
    assert_eq!(FactType::Other.default_path(), "aleph://knowledge/");
    assert_eq!(FactType::SubagentRun.default_path(), "aleph://agent/experiences/");
}
