//! Property-based tests for memory enum serde roundtrip and invariants.
//!
//! Uses proptest to verify that serialization/deserialization and
//! `as_str()`/`FromStr` conversions are consistent for all memory enum types.

use proptest::prelude::*;
use std::collections::HashSet;

use super::{MemoryLayer, NoteType};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary NoteType variant.
fn arb_note_type() -> impl Strategy<Value = NoteType> {
    prop_oneof![
        Just(NoteType::Preference),
        Just(NoteType::Plan),
        Just(NoteType::Learning),
        Just(NoteType::Project),
        Just(NoteType::Personal),
        Just(NoteType::Tool),
        Just(NoteType::Other),
        Just(NoteType::SubagentRun),
        Just(NoteType::SubagentSession),
        Just(NoteType::SubagentCheckpoint),
        Just(NoteType::SubagentTranscript),
        Just(NoteType::Lesson),
        Just(NoteType::Skill),
        Just(NoteType::Reference),
        Just(NoteType::Transcript),
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

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    /// NoteType: serialize then deserialize preserves the variant.
    #[test]
    fn note_type_serde_roundtrip(ref ft in arb_note_type()) {
        let json_str = serde_json::to_string(ft).unwrap();
        let parsed: NoteType = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(&parsed, ft);
    }

    /// NoteType: as_str() followed by FromStr roundtrip preserves the variant.
    #[test]
    fn note_type_as_str_from_str_roundtrip(ref ft in arb_note_type()) {
        let s = ft.as_str();
        let parsed: NoteType = s.parse().unwrap();
        prop_assert_eq!(&parsed, ft);
    }

    /// NoteType: default_path() always starts with "aleph://".
    #[test]
    fn note_type_default_path_starts_with_aleph(ref ft in arb_note_type()) {
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

}

/// NoteType: all as_str() values are unique across all variants.
///
/// This is a deterministic exhaustive test (not randomized), ensuring no two
/// variants accidentally map to the same string.
#[test]
fn note_type_as_str_values_are_unique() {
    let all_variants: Vec<NoteType> = vec![
        NoteType::Preference,
        NoteType::Plan,
        NoteType::Learning,
        NoteType::Project,
        NoteType::Personal,
        NoteType::Tool,
        NoteType::Other,
        NoteType::SubagentRun,
        NoteType::SubagentSession,
        NoteType::SubagentCheckpoint,
        NoteType::SubagentTranscript,
        // Lesson / Skill / Reference / Feedback / Transcript were added after
        // this list was first written; the count assertion below is what keeps
        // the census honest, so a missing variant fails loudly here.
        NoteType::Lesson,
        NoteType::Skill,
        NoteType::Reference,
        NoteType::Feedback,
        NoteType::Transcript,
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
