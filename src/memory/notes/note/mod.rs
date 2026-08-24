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

pub use helpers::{sanitize_note_path, sanitize_title};
pub use parsing::{fact_provenance_for, ExtraFrontmatter};
pub use relation::{is_structural_strong, Relation, STRUCTURAL_STRONG};
pub use types::{FactProvenance, ProvenanceOrigin, Severity};

use helpers::{sha256_hex, yaml_extra_block, yaml_inline_array, yaml_scalar};
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
    /// Bullet points from the body (lines starting with `- `).
    ///
    /// When `body` is `Some`, this is a derived *index view* — mutate it via
    /// [`KnowledgeNote::append_facts`] / [`KnowledgeNote::set_body`] so the
    /// verbatim body stays in sync; a direct `facts` push would silently be
    /// dropped by `to_markdown` (the body wins).
    pub facts: Vec<String>,
    /// Extracted `[[wikilinks]]` from the body. Same sync rule as `facts`:
    /// use [`KnowledgeNote::add_links`] when `body` is `Some`.
    pub links: Vec<String>,
    /// Verbatim markdown body (everything after the closing `---` fence).
    ///
    /// `Some` for notes parsed from disk — `to_markdown` then re-emits it
    /// byte-for-byte under regenerated frontmatter, so prose / headings /
    /// code blocks survive every round-trip. `None` for programmatically
    /// constructed notes — `to_markdown` falls back to the legacy
    /// facts-bullets + `Related:` rendering (byte-identical to the
    /// pre-body-field output).
    #[serde(default)]
    pub body: Option<String>,
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
    /// `NoteDrift`'s outdated/contradicted verdict (frontmatter `stale: true`).
    /// Parse-only: `to_markdown` never emits it (matching the pre-field
    /// drop-on-rewrite behaviour), so byte-preserving raw writers — `NoteDrift`
    /// (which sets it) and `NoteDecay`'s `write_note_raw` — carry it while
    /// lossy full rewrites drop it exactly as before. Read by `NoteDecay` to
    /// archive stale notes out of active retrieval. `false` for legacy notes.
    #[serde(default)]
    pub stale: bool,
    /// Obsidian / llm_wiki page-type (mirrors category). `None` for legacy notes.
    /// Single source of truth remains the directory; this field enables vault
    /// byte-compatibility with Obsidian and llm_wiki readers.
    pub note_type: Option<String>,
    /// Obsidian aliases from frontmatter `aliases:`. Empty for legacy notes.
    pub aliases: Vec<String>,
    /// Frontmatter keys this layer does not model, preserved verbatim.
    ///
    /// `to_markdown` regenerates the header from the typed fields above, so
    /// without this every key a human / Obsidian plugin / external tool wrote
    /// (`cssclass`, `publish`, `id`, project-specific keys…) was destroyed by
    /// the first write that passed through the note layer — the markdown-is-
    /// source-of-truth contract held for the body and not for the header.
    ///
    /// Never contains a key in
    /// [`parsing::KNOWN_FRONTMATTER_KEYS`]; empty for notes whose header this
    /// layer fully models, which then serialize byte-for-byte as before.
    #[serde(default)]
    pub extra_frontmatter: ExtraFrontmatter,
}

impl Default for KnowledgeNote {
    fn default() -> Self {
        Self {
            title: String::new(),
            category: String::new(),
            tags: Vec::new(),
            facts: Vec::new(),
            links: Vec::new(),
            body: None,
            relations: Vec::new(),
            created_at: 0,
            updated_at: 0,
            content_hash: String::new(),
            confidence: 1.0,
            severity: types::Severity::Low,
            source_notes: Vec::new(),
            supersedes: Vec::new(),
            superseded_by: Vec::new(),
            fact_provenance: Vec::new(),
            permanent: false,
            stale: false,
            note_type: None,
            aliases: Vec::new(),
            extra_frontmatter: ExtraFrontmatter::new(),
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

        let (frontmatter, extra_frontmatter, body) = split_frontmatter(content)?;

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
            body: Some(body),
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
            supersedes: frontmatter.supersedes,
            superseded_by: frontmatter.superseded_by,
            fact_provenance,
            permanent: frontmatter.permanent,
            stale: frontmatter.stale,
            note_type: frontmatter.note_type,
            aliases: frontmatter.aliases,
            extra_frontmatter,
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
        // RFC3339 second precision: day-granular dates collapsed updated_at
        // to midnight on every reparse, breaking recency ordering after a
        // rebuild. `parse_date_to_unix` accepts both this and legacy
        // YYYY-MM-DD.
        let created = DateTime::from_timestamp(self.created_at, 0)
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default();
        let updated = DateTime::from_timestamp(self.updated_at, 0)
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default();

        let mut out = String::new();
        out.push_str("---\n");
        // Vault-compat header (Obsidian / llm_wiki): type/title/aliases.
        // Scalars are quoted when needed — an unquoted `title: [wip] plans`
        // makes the whole note permanently unparseable.
        let note_type = self
            .note_type
            .clone()
            .unwrap_or_else(|| self.category.clone());
        out.push_str(&format!("type: {}\n", yaml_scalar(&note_type)));
        out.push_str(&format!("title: {}\n", yaml_scalar(&self.title)));
        out.push_str(&format!("aliases: {}\n", yaml_inline_array(&self.aliases)));
        out.push_str(&format!("category: {}\n", yaml_scalar(&self.category)));
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
                // Both values are unvalidated model input (`to` may be a path or
                // a wikilink target; `rel_type` is a free-form LLM-chosen verb,
                // R7). They need the same quoting as every other scalar above —
                // an unquoted `to: [[plan/x]]` parses as a nested flow sequence
                // and makes the whole note permanently unparseable.
                out.push_str(&format!("  - to: {}\n", yaml_scalar(&r.to)));
                out.push_str(&format!("    type: {}\n", yaml_scalar(&r.rel_type)));
                out.push_str(&format!("    confidence: {:.4}\n", r.confidence));
            }
        }
        // Only emit `permanent:` when set, so non-permanent notes serialize
        // byte-for-byte as before this field existed.
        if self.permanent {
            out.push_str("permanent: true\n");
        }
        // Passthrough keys last, in deterministic (BTreeMap) order. Empty for
        // notes this layer fully models → byte-identical legacy output.
        out.push_str(&yaml_extra_block(&self.extra_frontmatter));
        out.push_str("---\n\n");

        if let Some(body) = self.body.as_deref() {
            // Verbatim body — prose / headings / code blocks survive the
            // round-trip. `facts` / `links` are derived index views here.
            out.push_str(body);
            if !body.is_empty() && !body.ends_with('\n') {
                out.push('\n');
            }
        } else {
            for fact in &self.facts {
                push_fact_bullet(&mut out, fact);
            }

            if !self.links.is_empty() {
                out.push('\n');
                let link_strs: Vec<String> =
                    self.links.iter().map(|l| format!("[[{l}]]")).collect();
                out.push_str(&format!("Related: {}\n", link_strs.join(" ")));
            }
        }

        out
    }

    /// Replace the verbatim body and re-derive the `facts` / `links` /
    /// `fact_provenance` index views from it.
    pub fn set_body(&mut self, content: impl Into<String>) {
        let content = content.into();
        self.facts = extract_facts(&content);
        self.links = extract_wikilinks(&content);
        self.fact_provenance = extract_provenance_markers(&content, &self.facts);
        self.body = Some(content);
    }

    /// Append bullet facts, keeping the verbatim body (when present) in sync
    /// so the new facts survive `to_markdown`.
    ///
    /// Facts land *above* the trailing `Related:` block. Appending blindly to
    /// the end interleaved bullets with the link footer, so a note touched by
    /// both the append path and the nightly link weaver degraded into
    /// alternating fact / `Related:` lines.
    pub fn append_facts(&mut self, new_facts: &[String]) {
        if new_facts.is_empty() {
            return;
        }
        if let Some(body) = self.body.as_mut() {
            let (mut head, related) = split_trailing_related(body);
            for fact in new_facts {
                if !head.is_empty() && !head.ends_with('\n') {
                    head.push('\n');
                }
                push_fact_bullet(&mut head, fact);
            }
            *body = join_body_and_related(&head, &related);
        }
        self.facts.extend(new_facts.iter().cloned());
    }

    /// Add wikilink targets (deduped against `links`), merging them into the
    /// body's single trailing `Related:` line for targets it doesn't already
    /// reference.
    ///
    /// Merging (rather than appending a fresh line each time) is what keeps a
    /// note readable: the nightly link weaver calls this once per weave, so a
    /// per-call `Related:` line meant a well-connected note grew one footer
    /// line per night. Consecutive trailing `Related:` lines left by the old
    /// behaviour are collapsed here on the next weave.
    pub fn add_links(&mut self, new_links: &[String]) {
        let fresh: Vec<String> = new_links
            .iter()
            .filter(|l| !self.links.contains(l))
            .cloned()
            .collect();
        if fresh.is_empty() {
            return;
        }
        if let Some(body) = self.body.as_mut() {
            let (head, mut related) = split_trailing_related(body);
            // Only the head is checked for an existing mention: a target
            // already named in the prose does not need a footer entry, but one
            // already in the footer is handled by the dedup below.
            let mut added_to_footer = false;
            for l in &fresh {
                if head.contains(&format!("[[{l}]]")) || head.contains(&format!("[[{l}|")) {
                    continue;
                }
                if related.iter().any(|existing| existing == l) {
                    continue;
                }
                related.push(l.clone());
                added_to_footer = true;
            }
            // A body whose footer needs no change must not be rewritten — an
            // unnecessary rewrite churns `content_hash` and, downstream, the
            // note's embedding.
            if added_to_footer {
                *body = join_body_and_related(&head, &related);
            }
        }
        self.links.extend(fresh);
    }

    /// Body text for embedding — the verbatim body when present, otherwise
    /// facts joined by newline (legacy view for programmatic notes).
    #[must_use]
    pub fn body_text(&self) -> String {
        match self.body.as_deref() {
            Some(body) if !body.is_empty() => body.to_string(),
            _ => self.facts.join("\n"),
        }
    }

    /// Body text for FTS indexing — facts joined by newline with inline
    /// `\u003c!-- src: ..., origin: ..., inferred: ... --\u003e` provenance markers
    /// stripped so they don't pollute the search index.
    ///
    /// Uses the full verbatim body when present — bullet-facts-only indexing
    /// made all prose in raw-written notes (panel edits, hand edits)
    /// invisible to the FTS leg of search.
    #[must_use]
    pub fn body_text_for_fts(&self) -> String {
        match self.body.as_deref() {
            Some(body) if !body.is_empty() => strip_provenance_marker(body),
            _ => self
                .facts
                .iter()
                .map(|f| strip_provenance_marker(f))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Each fact as a reader sees it — marker removed — beside the provenance
    /// that marker carried.
    ///
    /// This is the readable form of the `<!-- src: ..., origin: ..., inferred:
    /// ... -->` markers the ingest prompt asks the model to attach. They are
    /// written on every fact and stripped from every rendering except the
    /// raw-file read behind `note_manage(action='get')`, which returns them
    /// buried in prose; this is the first form that separates the two.
    ///
    /// Read from the note, not from `notes_provenance`: the text and the marker
    /// then come from the same bytes. Pairing text from one source with
    /// provenance from another is how a fact gets attributed to a source it
    /// never had.
    ///
    /// `fact_provenance` is a public field, so a caller can hand us a note
    /// whose two vectors disagree in length; a fact with no row falls back to
    /// `Legacy`, which is what "no marker" already means.
    #[must_use]
    pub fn facts_with_origin(&self) -> Vec<(String, FactProvenance)> {
        self.facts
            .iter()
            .enumerate()
            .map(|(i, f)| {
                (
                    strip_provenance_marker(f),
                    self.fact_provenance.get(i).cloned().unwrap_or_default(),
                )
            })
            .collect()
    }
}

/// Drop inline provenance markers from note text. Three call sites wanted the
/// same two lines; the third is where a spelling starts to drift.
fn strip_provenance_marker(s: &str) -> String {
    parsing::PROVENANCE_RE.replace_all(s, "").trim().to_string()
}

/// Marker that opens a note body's link footer.
const RELATED_PREFIX: &str = "Related:";

/// Split a body into its prose head and the wikilink targets of the trailing
/// `Related:` footer.
///
/// Only a *trailing* run of `Related:` lines counts, so the word appearing
/// mid-prose is never consumed. A run (rather than a single line) is accepted
/// because notes written before [`KnowledgeNote::add_links`] merged footers
/// accumulated one line per weave; those collapse on the next write.
///
/// The returned head carries no trailing newline — [`join_body_and_related`]
/// is the inverse and re-establishes the separator.
fn split_trailing_related(body: &str) -> (String, Vec<String>) {
    let mut lines: Vec<&str> = body.lines().collect();
    let mut footer_lines: Vec<&str> = Vec::new();

    while let Some(last) = lines.last() {
        let trimmed = last.trim();
        if trimmed.is_empty() {
            // Blank lines are only consumed as separators *inside* a footer we
            // have already started collecting; a body merely ending in blanks
            // keeps them.
            if footer_lines.is_empty() {
                break;
            }
            lines.pop();
            continue;
        }
        if trimmed.starts_with(RELATED_PREFIX) {
            footer_lines.push(trimmed);
            lines.pop();
            continue;
        }
        break;
    }

    // Restore document order (the scan ran backwards) before extracting.
    footer_lines.reverse();
    let mut targets: Vec<String> = Vec::new();
    for line in footer_lines {
        for target in extract_wikilinks(line) {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    (lines.join("\n"), targets)
}

/// Inverse of [`split_trailing_related`]: reattach a single `Related:` footer.
///
/// An empty target list returns the head unchanged, so bodies that never had a
/// footer round-trip byte-for-byte.
fn join_body_and_related(head: &str, targets: &[String]) -> String {
    if targets.is_empty() {
        return head.to_string();
    }
    let rendered: Vec<String> = targets.iter().map(|t| format!("[[{t}]]")).collect();
    let footer = format!("{RELATED_PREFIX} {}\n", rendered.join(" "));
    if head.is_empty() {
        return footer;
    }
    let separator = if head.ends_with('\n') { "\n" } else { "\n\n" };
    format!("{head}{separator}{footer}")
}

/// Append one fact as a `- ` bullet, indenting continuation lines by two
/// spaces so `extract_facts` re-attaches them on the next parse (unindented
/// tails were silently dropped on round-trip).
fn push_fact_bullet(out: &mut String, fact: &str) {
    let mut lines = fact.lines();
    let Some(first) = lines.next() else {
        return;
    };
    out.push_str(&format!("- {first}\n"));
    for cont in lines {
        if !cont.starts_with("  ") {
            out.push_str("  ");
        }
        out.push_str(cont);
        out.push('\n');
    }
}
