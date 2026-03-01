//! Property-based tests for memory enum serde roundtrip and invariants.
//!
//! Uses proptest to verify that serialization/deserialization and
//! `as_str()`/`FromStr` conversions are consistent for all memory enum types.

use proptest::prelude::*;
use std::collections::HashSet;

use super::{FactType, MemoryLayer, MemoryScope, MemoryTier};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary FactType variant.
fn arb_fact_type() -> impl Strategy<Value = FactType> {
    prop_oneof![
        Just(FactType::Preference),
        Just(FactType::Plan),
        Just(FactType::Learning),
        Just(FactType::Project),
        Just(FactType::Personal),
        Just(FactType::Tool),
        Just(FactType::Other),
        Just(FactType::SubagentRun),
        Just(FactType::SubagentSession),
        Just(FactType::SubagentCheckpoint),
        Just(FactType::SubagentTranscript),
    ]
}

/// Generate an arbitrary MemoryLayer variant.
fn arb_memory_layer() -> impl Strategy<Value = MemoryLayer> {
    prop_oneof![
        Just(MemoryLayer::L0Abstract),
        Just(MemoryLayer::L1Overview),
        Just(MemoryLayer::L2Detail),
    ]
}

/// Generate an arbitrary MemoryTier variant.
fn arb_memory_tier() -> impl Strategy<Value = MemoryTier> {
    prop_oneof![
        Just(MemoryTier::Core),
        Just(MemoryTier::ShortTerm),
        Just(MemoryTier::LongTerm),
    ]
}

/// Generate an arbitrary MemoryScope variant.
fn arb_memory_scope() -> impl Strategy<Value = MemoryScope> {
    prop_oneof![
        Just(MemoryScope::Global),
        Just(MemoryScope::Workspace),
        Just(MemoryScope::Persona),
    ]
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    /// FactType: serialize then deserialize preserves the variant.
    #[test]
    fn fact_type_serde_roundtrip(ref ft in arb_fact_type()) {
        let json_str = serde_json::to_string(ft).unwrap();
        let parsed: FactType = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(&parsed, ft);
    }

    /// FactType: as_str() followed by FromStr roundtrip preserves the variant.
    #[test]
    fn fact_type_as_str_from_str_roundtrip(ref ft in arb_fact_type()) {
        let s = ft.as_str();
        let parsed: FactType = s.parse().unwrap();
        prop_assert_eq!(&parsed, ft);
    }

    /// FactType: default_path() always starts with "aleph://".
    #[test]
    fn fact_type_default_path_starts_with_aleph(ref ft in arb_fact_type()) {
        let path = ft.default_path();
        prop_assert!(
            path.starts_with("aleph://"),
            "Expected default_path to start with \"aleph://\", got: {}",
            path
        );
    }

    /// MemoryLayer: serialize then deserialize preserves the variant.
    #[test]
    fn memory_layer_serde_roundtrip(ref layer in arb_memory_layer()) {
        let json_str = serde_json::to_string(layer).unwrap();
        let parsed: MemoryLayer = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(&parsed, layer);
    }

    /// MemoryTier: serialize then deserialize preserves the variant.
    #[test]
    fn memory_tier_serde_roundtrip(ref tier in arb_memory_tier()) {
        let json_str = serde_json::to_string(tier).unwrap();
        let parsed: MemoryTier = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(&parsed, tier);
    }

    /// MemoryScope: serialize then deserialize preserves the variant.
    #[test]
    fn memory_scope_serde_roundtrip(ref scope in arb_memory_scope()) {
        let json_str = serde_json::to_string(scope).unwrap();
        let parsed: MemoryScope = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(&parsed, scope);
    }
}

/// FactType: all as_str() values are unique across all variants.
///
/// This is a deterministic exhaustive test (not randomized), ensuring no two
/// variants accidentally map to the same string.
#[test]
fn fact_type_as_str_values_are_unique() {
    let all_variants: Vec<FactType> = vec![
        FactType::Preference,
        FactType::Plan,
        FactType::Learning,
        FactType::Project,
        FactType::Personal,
        FactType::Tool,
        FactType::Other,
        FactType::SubagentRun,
        FactType::SubagentSession,
        FactType::SubagentCheckpoint,
        FactType::SubagentTranscript,
    ];

    let mut seen = HashSet::new();
    for variant in &all_variants {
        let s = variant.as_str();
        assert!(
            seen.insert(s),
            "Duplicate as_str() value {:?} for variant {:?}",
            s,
            variant
        );
    }

    // Verify we checked all variants (count must match)
    assert_eq!(
        seen.len(),
        all_variants.len(),
        "Number of unique as_str() values should equal number of variants"
    );
}
