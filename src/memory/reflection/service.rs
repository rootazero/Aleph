//! Reflection service — gate logic and session-end orchestration.

use crate::config::types::memory::ReflectionConfig;
use crate::memory::context::MemoryFact;

use super::mapper::map_to_facts;
use super::parser::parse_reflection;

/// Determine whether a session-end reflection should be triggered.
///
/// Returns `false` if any gate criterion is not met:
/// - reflection disabled in config
/// - too few turns
/// - too few user characters
/// - cooldown not elapsed since last reflection
pub fn should_reflect(
    turn_count: u32,
    total_user_chars: u32,
    last_reflection_minutes_ago: Option<u32>,
    config: &ReflectionConfig,
) -> bool {
    if !config.enabled {
        return false;
    }
    if turn_count < config.min_turns {
        return false;
    }
    if total_user_chars < config.min_user_chars {
        return false;
    }
    if let Some(minutes_ago) = last_reflection_minutes_ago {
        if minutes_ago < config.cooldown_minutes {
            return false;
        }
    }
    true
}

/// Parse LLM reflection output and convert to memory facts + open loops.
///
/// Steps:
/// 1. Parse the markdown reflection into structured categories
/// 2. Map parsed items to `MemoryFact` via the mapper module
/// 3. Return (facts, open_loops)
pub fn process_reflection(
    llm_output: &str,
    _already_extracted: &[String],
) -> (Vec<MemoryFact>, Vec<String>) {
    let parsed = parse_reflection(llm_output);
    let facts = map_to_facts(&parsed);
    (facts, parsed.open_loops)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_enabled() -> ReflectionConfig {
        ReflectionConfig {
            enabled: true,
            min_turns: 5,
            min_user_chars: 200,
            cooldown_minutes: 30,
            open_loop_tracking: false,
            open_loop_inject_prompt: false,
        }
    }

    fn config_disabled() -> ReflectionConfig {
        ReflectionConfig {
            enabled: false,
            ..config_enabled()
        }
    }

    #[test]
    fn gate_rejects_when_disabled() {
        assert!(!should_reflect(10, 500, None, &config_disabled()));
    }

    #[test]
    fn gate_rejects_too_few_turns() {
        assert!(!should_reflect(3, 500, None, &config_enabled()));
    }

    #[test]
    fn gate_rejects_too_few_chars() {
        assert!(!should_reflect(10, 50, None, &config_enabled()));
    }

    #[test]
    fn gate_rejects_cooldown() {
        // Last reflection was 10 minutes ago, cooldown is 30
        assert!(!should_reflect(10, 500, Some(10), &config_enabled()));
    }

    #[test]
    fn gate_passes_when_all_criteria_met() {
        assert!(should_reflect(10, 500, Some(60), &config_enabled()));
    }

    #[test]
    fn gate_passes_no_prior_reflection() {
        // None means no prior reflection — cooldown does not apply
        assert!(should_reflect(10, 500, None, &config_enabled()));
    }

    #[test]
    fn process_returns_facts_and_open_loops() {
        let md = "\
## Invariants
- User prefers Chinese dialogue

## Derived
- Session focused on memory optimization

## Lessons
- UTF-8 slicing: byte index panics → use char_indices

## Open Loops
- Finish compression daemon tuning
";
        let (facts, open_loops) = process_reflection(md, &[]);
        // 1 invariant + 1 derived + 1 lesson = 3 facts
        assert_eq!(facts.len(), 3);
        assert_eq!(open_loops.len(), 1);
        assert_eq!(open_loops[0], "Finish compression daemon tuning");

        // Check fact content
        assert_eq!(facts[0].content, "User prefers Chinese dialogue");
        assert_eq!(facts[1].content, "Session focused on memory optimization");
        assert!(facts[2].content.contains("UTF-8 slicing"));
    }

    #[test]
    fn process_reflection_includes_skills_as_facts() {
        use crate::memory::context::{FactType, MemoryScope, MemoryTier};

        let md = "\
## Invariants
- User prefers dark mode

## Derived
- (none)

## Lessons
- (none)

## Skills
- Cross-session FTS5 search: build FTS5 index on messages, group by session, return context
- Atomic file writes: use tempfile + rename to prevent corruption

## Open Loops
- Check deployment status
";
        let (facts, open_loops) = process_reflection(md, &[]);
        // 1 invariant + 2 skills = 3 facts (derived/lessons are "(none)")
        assert_eq!(facts.len(), 3);
        assert_eq!(open_loops.len(), 1);

        // Skills should be the last 2 facts
        let skill_facts: Vec<_> = facts
            .iter()
            .filter(|f| f.fact_type == FactType::Lesson && f.path.contains("skills/"))
            .collect();
        assert_eq!(skill_facts.len(), 2, "Expected 2 skill facts");

        for sf in &skill_facts {
            assert_eq!(sf.tier, MemoryTier::LongTerm);
            assert_eq!(sf.scope, MemoryScope::Global);
            assert!(sf.path.starts_with("aleph://knowledge/skills/"));
            assert!((sf.confidence - 0.85).abs() < f32::EPSILON);
        }

        // Verify slug generation in paths
        assert!(skill_facts.iter().any(|f| f.path.contains("cross-session-fts5-search")));
        assert!(skill_facts.iter().any(|f| f.path.contains("atomic-file-writes")));
    }
}
