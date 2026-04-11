//! Skill recall — formats retrieved skill facts into a system prompt fragment.

use crate::memory::context::MemoryFact;
use crate::skill::tools::search::skill_name_from_path;

/// Maximum number of skills injected per conversation by default.
pub const DEFAULT_SKILL_RECALL_LIMIT: usize = 5;

/// Format a slice of skill [`MemoryFact`]s into a system prompt fragment.
///
/// Returns `None` when `skills` is empty so callers can skip injection entirely.
pub fn format_skills_prompt(skills: &[MemoryFact]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }

    let mut out = String::from(
        "## Learned Skills (auto-retrieved)\n\
         The following skills were learned from past sessions and may be relevant.\n\
         Follow them if applicable. If a skill is outdated, update it via skill_manage.\n",
    );

    for fact in skills {
        let name = skill_name_from_path(&fact.path).unwrap_or("unknown");
        out.push('\n');
        out.push_str(&format!("### {name}\n"));
        out.push_str(&fact.content);
        out.push('\n');
    }

    Some(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::NoteType;

    fn make_skill_fact(name: &str, content: &str) -> MemoryFact {
        let path = format!("aleph://skills/coding/{}/", name);
        let mut fact = MemoryFact::new(content.to_string(), NoteType::Skill, Vec::new());
        fact.path = path;
        fact
    }

    #[test]
    fn formats_empty_skills_as_none() {
        assert_eq!(format_skills_prompt(&[]), None);
    }

    #[test]
    fn formats_single_skill() {
        let fact = make_skill_fact("rust-lifetimes", "Always annotate lifetimes explicitly.");
        let result = format_skills_prompt(&[fact]).unwrap();
        assert!(result.contains("### rust-lifetimes"), "should contain skill name header");
        assert!(
            result.contains("Always annotate lifetimes explicitly."),
            "should contain skill content"
        );
    }

    #[test]
    fn formats_multiple_skills() {
        let facts = vec![
            make_skill_fact("rust-lifetimes", "Always annotate lifetimes explicitly."),
            make_skill_fact("error-handling", "Use thiserror for library errors."),
        ];
        let result = format_skills_prompt(&facts).unwrap();
        assert!(result.contains("### rust-lifetimes"), "should contain first skill name");
        assert!(result.contains("### error-handling"), "should contain second skill name");
    }
}
