//! Typed relation edges for entity-graph notes (Gap A).
//!
//! Encoded in note frontmatter under `relations:`; mirrored into the
//! rebuildable `notes_links.relation` index column. Markdown is the source of
//! truth — these structs are reconstructed from the `.md` file on every parse.

use serde::{Deserialize, Serialize};

/// A typed, directed edge from the containing note to `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    /// Target note path ("entity/bob") or raw wikilink target.
    pub to: String,
    /// Free-form snake_case relationship verb chosen by the LLM (no fixed
    /// taxonomy — R7 LLM sovereignty). E.g. "works_at", "colleague".
    #[serde(rename = "type")]
    pub rel_type: String,
    /// LLM-judged edge confidence in [0,1]; defaults to 1.0 when absent.
    #[serde(default = "default_relation_confidence")]
    pub confidence: f32,
}

pub(crate) fn default_relation_confidence() -> f32 {
    1.0
}

impl Relation {
    /// Clamp `confidence` into `[0,1]` (P7 boundary hardening). Applied when a
    /// relation enters the system from markdown or from an ingest op.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self
    }
}
