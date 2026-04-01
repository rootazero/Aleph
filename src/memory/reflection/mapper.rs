//! Maps parsed reflection output to MemoryFact instances.

use super::parser::{LessonItem, ReflectionOutput};
use crate::memory::context::{
    FactSource, FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};

/// Classify an invariant text as Preference or Personal based on keywords.
pub fn classify_invariant(text: &str) -> FactType {
    let lower = text.to_lowercase();
    let preference_keywords = ["prefer", "like", "偏好", "喜欢"];
    if preference_keywords.iter().any(|kw| lower.contains(kw)) {
        FactType::Preference
    } else {
        FactType::Personal
    }
}

/// Format a lesson item as "symptom: cause → resolution".
///
/// Falls back to just the symptom if cause and resolution are empty.
pub fn format_lesson(lesson: &LessonItem) -> String {
    if lesson.cause.is_empty() && lesson.resolution.is_empty() {
        return lesson.symptom.clone();
    }
    if lesson.cause.is_empty() {
        return format!("{} → {}", lesson.symptom, lesson.resolution);
    }
    if lesson.resolution.is_empty() {
        return format!("{}: {}", lesson.symptom, lesson.cause);
    }
    format!(
        "{}: {} → {}",
        lesson.symptom, lesson.cause, lesson.resolution
    )
}

/// Convert a parsed reflection into a list of MemoryFact entries.
///
/// Mapping rules:
/// - Invariants → Core tier, Preference/Personal type, confidence 0.85, L1Overview
/// - Derived → ShortTerm tier, Other type, confidence 0.70, L2Detail
/// - Lessons → LongTerm tier, Lesson type, confidence 0.80, L1Overview, Cases category
/// - Open Loops → skipped (actions, not facts)
pub fn map_to_facts(output: &ReflectionOutput) -> Vec<MemoryFact> {
    let mut facts = Vec::new();

    // Invariants → Core tier
    for text in &output.invariants {
        let fact_type = classify_invariant(text);
        let fact = MemoryFact::new(text.clone(), fact_type, Vec::new())
            .with_confidence(0.85)
            .with_tier(MemoryTier::Core)
            .with_layer(MemoryLayer::L1Overview)
            .with_fact_source(FactSource::Extracted);
        facts.push(fact);
    }

    // Derived → ShortTerm tier
    for text in &output.derived {
        let fact = MemoryFact::new(text.clone(), FactType::Other, Vec::new())
            .with_confidence(0.70)
            .with_tier(MemoryTier::ShortTerm)
            .with_layer(MemoryLayer::L2Detail)
            .with_fact_source(FactSource::Extracted);
        facts.push(fact);
    }

    // Lessons → LongTerm tier
    for lesson in &output.lessons {
        let content = format_lesson(lesson);
        let fact = MemoryFact::new(content, FactType::Lesson, Vec::new())
            .with_confidence(0.80)
            .with_tier(MemoryTier::LongTerm)
            .with_layer(MemoryLayer::L1Overview)
            .with_category(MemoryCategory::Cases)
            .with_fact_source(FactSource::Extracted);
        facts.push(fact);
    }

    // Skills → LongTerm tier, Lesson type, skills/ path prefix
    for text in &output.skills {
        let slug = text
            .split(':')
            .next()
            .unwrap_or(text)
            .trim()
            .to_lowercase()
            .replace(' ', "-")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>();
        let path = format!("aleph://knowledge/skills/{}/", slug);
        let fact = MemoryFact::new(text.clone(), FactType::Lesson, Vec::new())
            .with_confidence(0.85)
            .with_tier(MemoryTier::LongTerm)
            .with_scope(MemoryScope::Global)
            .with_layer(MemoryLayer::L1Overview)
            .with_category(MemoryCategory::Patterns)
            .with_path(path)
            .with_fact_source(FactSource::Extracted);
        facts.push(fact);
    }

    // Open Loops are intentionally skipped — they represent actions, not facts.

    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::reflection::parser::parse_reflection;

    #[test]
    fn maps_all_categories() {
        let md = "\
## Invariants
- User prefers Chinese dialogue

## Derived
- Session focused on memory optimization

## Lessons
- UTF-8 slicing: byte index panics on CJK → use char_indices

## Open Loops
- Finish compression daemon tuning
";
        let output = parse_reflection(md);
        let facts = map_to_facts(&output);

        // Open loops excluded → 3 facts total
        assert_eq!(facts.len(), 3);

        // Invariant: "prefers" keyword → Preference type, Core tier
        assert_eq!(facts[0].fact_type, FactType::Preference);
        assert_eq!(facts[0].tier, MemoryTier::Core);
        assert!((facts[0].confidence - 0.85).abs() < f32::EPSILON);
        assert_eq!(facts[0].layer, MemoryLayer::L1Overview);

        // Derived: Other type, ShortTerm tier
        assert_eq!(facts[1].fact_type, FactType::Other);
        assert_eq!(facts[1].tier, MemoryTier::ShortTerm);
        assert!((facts[1].confidence - 0.70).abs() < f32::EPSILON);
        assert_eq!(facts[1].layer, MemoryLayer::L2Detail);

        // Lesson: Lesson type, LongTerm tier, Cases category
        assert_eq!(facts[2].fact_type, FactType::Lesson);
        assert_eq!(facts[2].tier, MemoryTier::LongTerm);
        assert!((facts[2].confidence - 0.80).abs() < f32::EPSILON);
        assert_eq!(facts[2].layer, MemoryLayer::L1Overview);
        assert_eq!(facts[2].category, MemoryCategory::Cases);
        assert_eq!(
            facts[2].content,
            "UTF-8 slicing: byte index panics on CJK → use char_indices"
        );
    }

    #[test]
    fn maps_skills_to_lesson_facts_with_skills_path() {
        let md = "\
## Skills
- Cross-session FTS5 search: build index, group by session, return context
";
        let output = parse_reflection(md);
        let facts = map_to_facts(&output);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact_type, FactType::Lesson);
        assert_eq!(facts[0].tier, MemoryTier::LongTerm);
        assert_eq!(facts[0].scope, crate::memory::context::MemoryScope::Global);
        assert!(facts[0].path.starts_with("aleph://knowledge/skills/"));
        assert!((facts[0].confidence - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn skills_slug_handles_special_characters() {
        let md = "\
## Skills
- Résumé parsing (v2.1): extract structured fields from PDF
- 数据库优化: use EXPLAIN ANALYZE before indexing
- multi--dash & symbols!: clean up edge cases
";
        let output = parse_reflection(md);
        let facts = map_to_facts(&output);

        assert_eq!(facts.len(), 3);

        // Non-ASCII stripped, colons split on first colon for slug
        let paths: Vec<&str> = facts.iter().map(|f| f.path.as_str()).collect();

        // "Résumé parsing (v2.1)" → slug: "rsum-parsing-v21"
        assert!(
            paths[0].starts_with("aleph://knowledge/skills/"),
            "path should have skills prefix: {}",
            paths[0]
        );
        // Slug should only contain ascii alphanumeric and hyphens
        let slug0 = paths[0]
            .strip_prefix("aleph://knowledge/skills/")
            .unwrap()
            .trim_end_matches('/');
        assert!(
            slug0.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "slug should be ascii-safe: {slug0}"
        );

        // Chinese characters stripped entirely → slug is empty or just hyphens
        // The slug for "数据库优化" (all non-ASCII) should still produce a valid path
        assert!(paths[1].starts_with("aleph://knowledge/skills/"));

        // Multi-dash and symbols
        let slug2 = paths[2]
            .strip_prefix("aleph://knowledge/skills/")
            .unwrap()
            .trim_end_matches('/');
        assert!(
            slug2.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "slug should be ascii-safe: {slug2}"
        );
    }

    #[test]
    fn empty_reflection_produces_no_facts() {
        let output = ReflectionOutput::default();
        let facts = map_to_facts(&output);
        assert!(facts.is_empty());
    }
}
