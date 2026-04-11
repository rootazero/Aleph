//! SkillSearchTool — semantic search over learned skills.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::context::NoteType;
use crate::memory::store::types::SearchFilter;

// ---------------------------------------------------------------------------
// Args / Result types
// ---------------------------------------------------------------------------

/// Arguments for the skill search tool.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SkillSearchArgs {
    /// Natural-language query describing the skill to look up.
    pub query: String,
    /// Optional scope prefix to restrict the search (e.g. "coding", "research").
    pub scope: Option<String>,
    /// Maximum number of results to return (defaults to 10).
    pub limit: Option<usize>,
}

/// A single skill search result.
#[derive(Debug, Clone, Serialize)]
pub struct SkillSearchResult {
    /// Human-readable skill name derived from the VFS path.
    pub name: String,
    /// Short description of what the skill does.
    pub description: String,
    /// Full VFS path to the skill fact (e.g. "aleph://skills/coding/rust-debug/").
    pub path: String,
    /// Relevance score in [0, 1] (higher is more relevant).
    pub relevance: f32,
}

// ---------------------------------------------------------------------------
// Filter builder
// ---------------------------------------------------------------------------

/// Build a [`SearchFilter`] that targets only valid skill facts.
pub fn build_skill_filter() -> SearchFilter {
    SearchFilter::new()
        .with_note_type(NoteType::Skill)
        .with_valid_only()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the skill name from a VFS path.
///
/// Examples:
/// - `"aleph://skills/coding/rust-debug/"` → `"rust-debug"`
/// - `"aleph://skills/research/"` → `"research"`
/// - `"aleph://skills/rust-debug"` → `"rust-debug"`
pub fn skill_name_from_path(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_skill_name_from_path() {
        assert_eq!(
            skill_name_from_path("aleph://skills/coding/rust-debug/"),
            Some("rust-debug")
        );
        assert_eq!(
            skill_name_from_path("aleph://skills/research/"),
            Some("research")
        );
        assert_eq!(
            skill_name_from_path("aleph://skills/rust-debug"),
            Some("rust-debug")
        );
        // Trailing slash only (degenerate case)
        assert_eq!(skill_name_from_path("/"), None);
        // Empty string
        assert_eq!(skill_name_from_path(""), None);
    }

    #[test]
    fn builds_filter_with_skill_type() {
        let filter = build_skill_filter();
        assert_eq!(filter.note_type, Some(NoteType::Skill));
        assert_eq!(filter.is_valid, Some(true));
    }
}
