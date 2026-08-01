//! Typed relation edges for entity-graph notes (Gap A).
//!
//! Encoded in note frontmatter under `relations:`; mirrored into the
//! rebuildable `notes_links.relation` index column. Markdown is the source of
//! truth — these structs are reconstructed from the `.md` file on every parse.

use serde::{Deserialize, Serialize};

/// A typed, directed edge from the containing note to `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// Target note path ("entity/bob"). On the ingest path `[P<n>]` page tokens
    /// are rewritten to canonical paths (or the entry dropped) by
    /// `RefTable::resolve_relations` before the relation is ever serialized.
    ///
    /// This is unvalidated model input, so `to_markdown` quotes it via
    /// `yaml_scalar` — an unquoted `to: [[x/y]]` is a YAML flow sequence and
    /// makes the entire note unparseable.
    pub to: String,
    /// Free-form `snake_case` relationship verb chosen by the LLM (no fixed
    /// taxonomy — R7 LLM sovereignty). E.g. "`works_at`", "colleague".
    #[serde(rename = "type")]
    pub rel_type: String,
    /// LLM-judged edge confidence in [0,1]; defaults to 1.0 when absent.
    #[serde(default = "default_relation_confidence")]
    pub confidence: f32,
}

pub(crate) const fn default_relation_confidence() -> f32 {
    1.0
}

impl Relation {
    /// Clamp `confidence` into `[0,1]` (P7 boundary hardening). Applied when a
    /// relation enters the system from markdown or from an ingest op.
    #[must_use]
    pub const fn clamped(mut self) -> Self {
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self
    }
}

/// The only relation verbs the system treats specially: their targets are
/// force-surfaced at retrieval regardless of score (missing a superseded or
/// contradicting note is a correctness bug). All other rel_types stay
/// LLM-chosen and untyped to the system (R7).
pub const STRUCTURAL_STRONG: &[&str] = &["supersedes", "superseded_by", "contradicts"];

#[must_use]
pub fn is_structural_strong(rel_type: &str) -> bool {
    STRUCTURAL_STRONG.contains(&rel_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_strong_membership() {
        assert!(is_structural_strong("contradicts"));
        assert!(is_structural_strong("superseded_by"));
        assert!(!is_structural_strong("works_at"));
        assert!(!is_structural_strong("CONTRADICTS")); // case-sensitive, snake_case only
    }
}
