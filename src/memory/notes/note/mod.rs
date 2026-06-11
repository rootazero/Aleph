//! `KnowledgeNote` — the primary memory unit backed by a markdown file.

use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;

use super::wikilink::extract_wikilinks;

mod helpers;
mod parsing;
mod relation;
#[cfg(test)]
mod tests;
pub mod types;

pub use helpers::sanitize_title;
pub use relation::Relation;
pub use types::{FactProvenance, ProvenanceOrigin, Severity};

use helpers::{sha256_hex, yaml_inline_array};
use parsing::{extract_facts, extract_provenance_markers, parse_date_to_unix, split_frontmatter};

/// Whether any tag marks a note as permanent core knowledge. Recognises
/// `permanent` and `pinned` (case-insensitive). Shared SSOT used by both
/// [`KnowledgeNote::is_permanent`] and the dream decay stage, which inspects
/// the lightweight index `tags` without loading note bodies.
#[must_use]
pub fn tags_mark_permanent(tags: &[String]) -> bool {
    tags.iter().any(|t| {
        let t = t.trim();
        t.eq_ignore_ascii_case("permanent") || t.eq_ignore_ascii_case("pinned")
    })
}

/// A knowledge note — the primary memory unit.
///
/// Parsed from (and serializable back to) a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNote {
    /// Filename without `.md` extension
    pub title: String,
    /// From frontmatter `category` field
    pub category: String,
    /// From frontmatter `tags` field
    pub tags: Vec<String>,
    /// Bullet points from the body (lines starting with `- `)
    pub facts: Vec<String>,
    /// Extracted `[[wikilinks]]` from the body
    pub links: Vec<String>,
    /// Typed relation edges from frontmatter `relations:` (Gap A entity graph).
    /// Empty for notes without a `relations:` block (legacy + non-entity notes).
    pub relations: Vec<Relation>,
    /// Unix timestamp (seconds) — from frontmatter `created` date
    pub created_at: i64,
    /// Unix timestamp (seconds) — from frontmatter `updated` date
    pub updated_at: i64,
    /// SHA-256 hex digest of the full file content
    pub content_hash: String,
    /// LLM-assigned distillation confidence; 1.0 for legacy notes (no
    /// `confidence:` in frontmatter). Used by retrieval re-rank.
    pub confidence: f32,
    /// LLM-judged importance; `Severity::Low` for legacy notes. Used by
    /// retrieval re-rank.
    pub severity: types::Severity,
    /// Source synthesis-note paths or raw-memory IDs that produced this note.
    /// Empty for hand-authored / legacy notes.
    pub source_notes: Vec<String>,
    /// Governance status. `Active` for legacy notes. Phase C2 supersession /
    /// contradiction handling sets this to `Deprecated` or `Contradicted`.
    pub status: types::NoteStatus,
    /// Note paths this note supersedes (i.e. this note replaces them).
    /// Empty for legacy / non-superseding notes.
    pub supersedes: Vec<String>,
    /// Note paths that supersede this note (i.e. they replace this one).
    /// Empty for legacy / non-superseded notes.
    pub superseded_by: Vec<String>,
    /// Per-fact provenance, one entry per item in `facts` (same order). Empty
    /// for legacy notes that lack inline `\u003c!-- src: ..., origin: ..., inferred: ... --\u003e`
    /// markers; otherwise populated by `extract_provenance_markers`.
    pub fact_provenance: Vec<FactProvenance>,
    /// Permanent core-knowledge marker. When `true`, the note is exempt from
    /// time-decay confidence erosion and low-activity archival (mirrors the
    /// `pinned` skill idiom in `skill_lifecycle`). `false` for legacy notes —
    /// see [`KnowledgeNote::is_permanent`] for the tag-based fallback.
    pub permanent: bool,
}

impl Default for KnowledgeNote {
    fn default() -> Self {
        Self {
            title: String::new(),
            category: String::new(),
            tags: Vec::new(),
            facts: Vec::new(),
            links: Vec::new(),
            relations: Vec::new(),
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
            confidence: 1.0,
            severity: types::Severity::Low,
            source_notes: Vec::new(),
            status: types::NoteStatus::default(),
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            fact_provenance: Vec::new(),
            permanent: false,
        }
    }
}

impl KnowledgeNote {
    /// Parse a markdown file into a `KnowledgeNote`.
    ///
    /// The `title` is typically the filename without `.md`.
    /// The `content` is the full file content (frontmatter + body).
    pub fn from_markdown(title: &str, content: &str) -> Result<Self, AlephError> {
        let content_hash = sha256_hex(content);

        let (frontmatter, body) = split_frontmatter(content)?;

        let created_at = parse_date_to_unix(&frontmatter.created)?;
        let updated_at = parse_date_to_unix(&frontmatter.updated)?;

        let facts = extract_facts(&body);
        let links = extract_wikilinks(&body);
        let fact_provenance = extract_provenance_markers(&body, &facts);

        Ok(Self {
            title: title.to_string(),
            category: frontmatter.category,
            tags: frontmatter.tags,
            facts,
            links,
            relations: frontmatter
                .relations
                .into_iter()
                .map(Relation::clamped)
                .collect(),
            created_at,
            updated_at,
            content_hash,
            confidence: frontmatter.confidence,
            severity: frontmatter.severity,
            source_notes: frontmatter.source_notes,
            status: frontmatter.status,
            supersedes: frontmatter.supersedes,
            superseded_by: frontmatter.superseded_by,
            fact_provenance,
            permanent: frontmatter.permanent,
        })
    }

    /// Whether this note is permanent core knowledge, exempt from decay and
    /// archival. True when the explicit `permanent: true` frontmatter flag is
    /// set, or — for notes authored before the flag existed — when the note
    /// carries a `permanent` / `pinned` tag. The tag fallback lets existing
    /// memory become permanent with zero migration.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        self.permanent || tags_mark_permanent(&self.tags)
    }

    /// Serialize this note back to markdown with YAML frontmatter.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let created = DateTime::from_timestamp(self.created_at, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let updated = DateTime::from_timestamp(self.updated_at, 0)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();

        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("category: {}\n", self.category));
        out.push_str(&format!("tags: {}\n", yaml_inline_array(&self.tags)));
        out.push_str(&format!("created: \"{created}\"\n"));
        out.push_str(&format!("updated: \"{updated}\"\n"));
        out.push_str(&format!("confidence: {:.4}\n", self.confidence));
        let severity_str = match self.severity {
            types::Severity::Low => "low",
            types::Severity::Med => "med",
            types::Severity::High => "high",
            types::Severity::Critical => "critical",
        };
        out.push_str(&format!("severity: {severity_str}\n"));
        out.push_str(&format!(
            "source_notes: {}\n",
            yaml_inline_array(&self.source_notes)
        ));
        let status_str = match self.status {
            types::NoteStatus::Active => "active",
            types::NoteStatus::Deprecated => "deprecated",
            types::NoteStatus::Contradicted => "contradicted",
        };
        out.push_str(&format!("status: {status_str}\n"));
        out.push_str(&format!(
            "supersedes: {}\n",
            yaml_inline_array(&self.supersedes)
        ));
        out.push_str(&format!(
            "superseded_by: {}\n",
            yaml_inline_array(&self.superseded_by)
        ));
        // Only emit `relations:` when non-empty, so notes without typed edges
        // serialize byte-for-byte as before this field existed (legacy parity).
        if !self.relations.is_empty() {
            out.push_str("relations:\n");
            for r in &self.relations {
                out.push_str(&format!("  - to: {}\n", r.to));
                out.push_str(&format!("    type: {}\n", r.rel_type));
                out.push_str(&format!("    confidence: {:.4}\n", r.confidence));
            }
        }
        // Only emit `permanent:` when set, so non-permanent notes serialize
        // byte-for-byte as before this field existed.
        if self.permanent {
            out.push_str("permanent: true\n");
        }
        out.push_str("---\n\n");

        for fact in &self.facts {
            out.push_str(&format!("- {fact}\n"));
        }

        if !self.links.is_empty() {
            out.push('\n');
            let link_strs: Vec<String> = self.links.iter().map(|l| format!("[[{l}]]")).collect();
            out.push_str(&format!("Related: {}\n", link_strs.join(" ")));
        }

        out
    }

    /// Body text for embedding — facts joined by newline.
    #[must_use]
    pub fn body_text(&self) -> String {
        self.facts.join("\n")
    }

    /// Body text for FTS indexing — facts joined by newline with inline
    /// `\u003c!-- src: ..., origin: ..., inferred: ... --\u003e` provenance markers
    /// stripped so they don't pollute the search index.
    #[must_use]
    pub fn body_text_for_fts(&self) -> String {
        self.facts
            .iter()
            .map(|f| parsing::PROVENANCE_RE.replace_all(f, "").trim().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
