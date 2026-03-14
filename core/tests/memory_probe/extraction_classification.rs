//! P1: Extraction & Classification — 8 scenarios

use alephcore::memory::context::{FactType, MemoryCategory, MemoryLayer, MemoryTier};
use alephcore::memory::reflection::mapper::{classify_invariant, map_to_facts};
use alephcore::memory::reflection::parser::parse_reflection;
use alephcore::memory::reflection::service::should_reflect;
use alephcore::MemoryConfig;

#[test]
fn p1_01_reflection_parses_all_four_sections() {
    let md = "\
## Invariants
- User prefers Chinese dialogue
- Rust core is the single source of truth

## Derived
- Session focused on memory optimization
- Token budget was tight

## Lessons
- UTF-8 slicing: byte index panics on CJK → use char_indices
- Lock poisoning: unwrap cascades panics → use unwrap_or_else

## Open Loops
- Finish compression daemon tuning
- Benchmark retrieval latency
- Add proptest for decay logic
";
    let out = parse_reflection(md);
    assert_eq!(out.invariants.len(), 2, "expected 2 invariants");
    assert_eq!(out.derived.len(), 2, "expected 2 derived");
    assert_eq!(out.lessons.len(), 2, "expected 2 lessons");
    assert_eq!(out.open_loops.len(), 3, "expected 3 open loops");
}

#[test]
fn p1_02_reflection_skips_placeholders_and_empty_lines() {
    let md = "\
## Invariants
- (none)
- Real invariant item
- (none captured)
-
- ()

## Derived
- (None)

## Lessons
- (NONE CAPTURED)
";
    let out = parse_reflection(md);
    // Only "Real invariant item" should survive
    assert_eq!(out.invariants.len(), 1);
    assert_eq!(out.invariants[0], "Real invariant item");
    assert!(out.derived.is_empty(), "placeholder derived should be filtered");
    assert!(out.lessons.is_empty(), "placeholder lessons should be filtered");
}

#[test]
fn p1_03_lesson_parses_symptom_cause_fix_format() {
    let md = "\
## Lessons
- UTF-8 slicing: byte index panics on CJK → use char_indices
";
    let out = parse_reflection(md);
    assert_eq!(out.lessons.len(), 1);
    let lesson = &out.lessons[0];
    assert_eq!(lesson.symptom, "UTF-8 slicing");
    assert_eq!(lesson.cause, "byte index panics on CJK");
    assert_eq!(lesson.resolution, "use char_indices");
}

#[test]
fn p1_04_lesson_fallback_unstructured_text() {
    let md = "\
## Lessons
- always test with real database
";
    let out = parse_reflection(md);
    assert_eq!(out.lessons.len(), 1);
    let lesson = &out.lessons[0];
    assert_eq!(lesson.symptom, "always test with real database");
    assert!(lesson.cause.is_empty(), "cause should be empty for unstructured text");
    assert!(
        lesson.resolution.is_empty(),
        "resolution should be empty for unstructured text"
    );
}

#[test]
fn p1_05_mapper_invariant_to_core_tier() {
    let md = "\
## Invariants
- User prefers dark mode
";
    let out = parse_reflection(md);
    let facts = map_to_facts(&out);
    assert_eq!(facts.len(), 1);
    let fact = &facts[0];

    // "prefers" keyword → Preference type
    assert_eq!(fact.fact_type, FactType::Preference);
    assert_eq!(fact.tier, MemoryTier::Core);
    assert!((fact.confidence - 0.85).abs() < f32::EPSILON);
    assert_eq!(fact.layer, MemoryLayer::L1Overview);

    // Also verify classify_invariant directly
    assert_eq!(classify_invariant("User prefers dark mode"), FactType::Preference);
    assert_eq!(classify_invariant("likes vim keybindings"), FactType::Preference);
    assert_eq!(classify_invariant("用户偏好中文"), FactType::Preference);
    assert_eq!(classify_invariant("用户喜欢 Rust"), FactType::Preference);
    assert_eq!(
        classify_invariant("Rust core is the single source of truth"),
        FactType::Personal
    );
}

#[test]
fn p1_06_mapper_derived_to_short_term_tier() {
    let md = "\
## Derived
- Session focused on memory optimization
";
    let out = parse_reflection(md);
    let facts = map_to_facts(&out);
    assert_eq!(facts.len(), 1);
    let fact = &facts[0];

    assert_eq!(fact.fact_type, FactType::Other);
    assert_eq!(fact.tier, MemoryTier::ShortTerm);
    assert!((fact.confidence - 0.70).abs() < f32::EPSILON);
    assert_eq!(fact.layer, MemoryLayer::L2Detail);
    assert_eq!(fact.content, "Session focused on memory optimization");
}

#[test]
fn p1_07_mapper_lesson_to_long_term_tier() {
    let md = "\
## Lessons
- UTF-8 slicing: byte index panics → use char_indices
";
    let out = parse_reflection(md);
    let facts = map_to_facts(&out);
    assert_eq!(facts.len(), 1);
    let fact = &facts[0];

    assert_eq!(fact.fact_type, FactType::Lesson);
    assert_eq!(fact.tier, MemoryTier::LongTerm);
    assert!((fact.confidence - 0.80).abs() < f32::EPSILON);
    assert_eq!(fact.layer, MemoryLayer::L1Overview);
    assert_eq!(fact.category, MemoryCategory::Cases);
    assert_eq!(
        fact.content,
        "UTF-8 slicing: byte index panics → use char_indices"
    );
}

#[test]
fn p1_08_reflection_gate_all_conditions() {
    // Build configs from MemoryConfig::default().reflection (ReflectionConfig is not re-exported)
    let mut cfg_enabled = MemoryConfig::default().reflection;
    cfg_enabled.enabled = true;
    cfg_enabled.min_turns = 5;
    cfg_enabled.min_user_chars = 200;
    cfg_enabled.cooldown_minutes = 30;

    let mut cfg_disabled = cfg_enabled.clone();
    cfg_disabled.enabled = false;

    // 1. disabled → false
    assert!(!should_reflect(10, 500, None, &cfg_disabled));

    // 2. too few turns (3 < 5) → false
    assert!(!should_reflect(3, 500, None, &cfg_enabled));

    // 3. too few chars (50 < 200) → false
    assert!(!should_reflect(10, 50, None, &cfg_enabled));

    // 4. cooldown not elapsed (10 < 30 minutes ago) → false
    assert!(!should_reflect(10, 500, Some(10), &cfg_enabled));

    // 5. all criteria met → true
    assert!(should_reflect(10, 500, Some(60), &cfg_enabled));

    // 6. None (no prior reflection) → cooldown doesn't apply → true
    assert!(should_reflect(10, 500, None, &cfg_enabled));
}
