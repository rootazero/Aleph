//! Reference-token indirection for the compound-ingest LLM loop.
//!
//! Mem0 maps real long identifiers (UUIDs) to small integer tokens before the
//! LLM call, then maps the model's integer references back to the real IDs.
//! The rationale: LLMs reliably reproduce `0`, `1`, `2` but drift when asked to
//! echo long structured strings verbatim. Aleph has the same exposure — the
//! compound-ingest planner is shown each related existing page by its full
//! `category/filename` path and must retype that path in every
//! `append`/`update`/`contradict`/`link`/`supersede` op. A single typo
//! (`preference` → `preferences`, a dropped `.md`, a normalised slug) made
//! [`apply::CompoundApplyTx::load_existing_or_default`] forge a brand-new
//! orphan page and report `appended += 1` as success — silent data loss.
//!
//! [`RefTable`] is the Rust analog of Mem0's mapping: related pages are shown
//! under stable `[P<n>]` tokens, and [`RefTable::resolve_plan`] rewrites every
//! token reference back to its exact canonical path via an O(1) table lookup
//! that cannot drift. A token that is out of range is, by construction, a
//! hallucinated reference; the offending op (or link entry) is dropped rather
//! than allowed to forge an orphan.
//!
//! This is pure identifier plumbing — the LLM still decides *which* pages to
//! touch and *what* facts to write. Resolution is tolerant: a raw path that is
//! not a token passes through unchanged, so pre-token planners (and the
//! existing test corpus) keep working byte-for-byte.

use crate::memory::notes::ingest::plan::{IngestPlan, PageOp};
use crate::memory::notes::ingest::retrieve::RelatedPage;
use crate::memory::notes::note::Relation;

/// Bidirectional map between the `[P<n>]` tokens shown to the LLM and the
/// canonical note paths of the related pages they stand for.
#[derive(Debug, Default, Clone)]
pub struct RefTable {
    /// Token index `n` → canonical note path (`category/filename`).
    paths: Vec<String>,
}

/// Tally of what [`RefTable::resolve_plan`] changed. Logged by the caller.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResolveStats {
    /// Token references rewritten to their canonical path.
    pub resolved: usize,
    /// Whole ops dropped because a primary target token was out of range.
    pub dropped_ops: usize,
    /// Individual link entries dropped because their token was out of range.
    pub dropped_links: usize,
}

/// Outcome of resolving a single identifier field.
enum FieldOutcome {
    /// Field was a valid in-range token; rewritten to the canonical path.
    Resolved,
    /// Field was a `[P<n>]` token but `n` is out of range — a hallucination.
    OutOfRange,
    /// Field was not a token (a raw path); left untouched.
    Passthrough,
}

impl RefTable {
    /// Build a table from the related pages, in the same order they are shown
    /// to the LLM (token `[P<i>]` ↔ `related[i].path`).
    #[must_use]
    pub fn from_related(related: &[RelatedPage]) -> Self {
        Self {
            paths: related.iter().map(|p| p.path.clone()).collect(),
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.paths.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The token string the prompt shows for related page index `idx`.
    #[must_use]
    pub fn token(idx: usize) -> String {
        format!("[P{idx}]")
    }

    /// Parse a `[P<n>]` token, returning the index when `value` (after trim) is
    /// exactly a token of that form. Anything else — including a path that
    /// merely contains brackets — returns `None`.
    fn parse_token(value: &str) -> Option<usize> {
        let inner = value.trim().strip_prefix("[P")?.strip_suffix(']')?;
        if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        inner.parse::<usize>().ok()
    }

    /// Resolve one identifier field in place.
    fn resolve_field(&self, value: &mut String) -> FieldOutcome {
        match Self::parse_token(value) {
            Some(idx) => match self.paths.get(idx) {
                Some(path) => {
                    *value = path.clone();
                    FieldOutcome::Resolved
                }
                None => FieldOutcome::OutOfRange,
            },
            None => FieldOutcome::Passthrough,
        }
    }

    /// Resolve a primary target field; `true` means the op is still valid,
    /// `false` means it referenced an out-of-range token and must be dropped.
    fn resolve_target(&self, value: &mut String, stats: &mut ResolveStats) -> bool {
        match self.resolve_field(value) {
            FieldOutcome::Resolved => {
                stats.resolved += 1;
                true
            }
            FieldOutcome::Passthrough => true,
            FieldOutcome::OutOfRange => false,
        }
    }

    /// Resolve a list of link references in place, dropping entries whose token
    /// is out of range (a hallucinated link target is silently discarded
    /// rather than written as a dangling edge).
    pub(crate) fn resolve_links(&self, links: &mut Vec<String>, stats: &mut ResolveStats) {
        links.retain_mut(|l| match self.resolve_field(l) {
            FieldOutcome::Resolved => {
                stats.resolved += 1;
                true
            }
            FieldOutcome::Passthrough => true,
            FieldOutcome::OutOfRange => {
                stats.dropped_links += 1;
                false
            }
        });
    }

    /// Resolve the `to` end of a typed relation list in place, dropping entries
    /// whose token is out of range. Same entry-drop policy as [`Self::resolve_links`]:
    /// a hallucinated edge target is discarded rather than written verbatim.
    ///
    /// This exists because the ingest prompt explicitly instructs the model to
    /// write `"to": "<entity path or [P<n>] token>"` (`prompts.rs`), so relation
    /// targets carry the same tokens links do. Without this the token reaches
    /// `Relation.to` untouched and is written into note frontmatter as a literal
    /// `[P0]` — the exact silent data loss this table exists to prevent.
    pub(crate) fn resolve_relations(
        &self,
        relations: &mut Vec<Relation>,
        stats: &mut ResolveStats,
    ) {
        relations.retain_mut(|r| match self.resolve_field(&mut r.to) {
            FieldOutcome::Resolved => {
                stats.resolved += 1;
                true
            }
            FieldOutcome::Passthrough => true,
            FieldOutcome::OutOfRange => {
                stats.dropped_links += 1;
                false
            }
        });
    }

    /// Rewrite every `[P<n>]` token in `plan` to its canonical path and drop
    /// ops that reference an out-of-range (hallucinated) page. Returns a tally
    /// for the caller to log. Raw-path fields are left untouched.
    ///
    /// Field policy:
    /// - `create.note_path` is a *new* page identifier — never resolved.
    /// - `create.links` / `append.new_links` are link lists — bad entries are
    ///   dropped individually (entry-drop).
    /// - `create.relations` / `append.new_relations` carry a `to` end that the
    ///   prompt explicitly allows to be a `[P<n>]` token — same entry-drop
    ///   policy as links.
    /// - all other path fields are primary targets — a bad token drops the
    ///   whole op (op-fatal), since the op has no valid page to act on.
    pub fn resolve_plan(&self, plan: &mut IngestPlan) -> ResolveStats {
        let mut stats = ResolveStats::default();
        let mut kept: Vec<PageOp> = Vec::with_capacity(plan.ops.len());

        for mut op in std::mem::take(&mut plan.ops) {
            let keep = match &mut op {
                PageOp::Create {
                    links, relations, ..
                } => {
                    self.resolve_links(links, &mut stats);
                    self.resolve_relations(relations, &mut stats);
                    true
                }
                PageOp::Append {
                    note_path,
                    new_links,
                    new_relations,
                    ..
                } => {
                    let ok = self.resolve_target(note_path, &mut stats);
                    if ok {
                        self.resolve_links(new_links, &mut stats);
                        self.resolve_relations(new_relations, &mut stats);
                    }
                    ok
                }
                PageOp::Update { note_path, .. } | PageOp::Contradict { note_path, .. } => {
                    self.resolve_target(note_path, &mut stats)
                }
                PageOp::Link { from, to } => {
                    // Resolve both ends; either may legitimately be a raw path
                    // for a page created earlier in the same batch.
                    let a = self.resolve_target(from, &mut stats);
                    let b = self.resolve_target(to, &mut stats);
                    a && b
                }
                PageOp::Supersede { old_path, new_path } => {
                    let a = self.resolve_target(old_path, &mut stats);
                    let b = self.resolve_target(new_path, &mut stats);
                    a && b
                }
            };

            if keep {
                kept.push(op);
            } else {
                stats.dropped_ops += 1;
            }
        }

        plan.ops = kept;
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(path: &str) -> RelatedPage {
        RelatedPage {
            path: path.to_string(),
            title: String::new(),
            summary: String::new(),
            content_preview: String::new(),
            tags: vec![],
            content_hash: String::new(),
            score: 0.0,
        }
    }

    fn table() -> RefTable {
        RefTable::from_related(&[
            page("preference/coding-style"),
            page("personal/li-wei"),
            page("projects/aleph"),
        ])
    }

    #[test]
    fn token_format_is_stable() {
        assert_eq!(RefTable::token(0), "[P0]");
        assert_eq!(RefTable::token(12), "[P12]");
    }

    #[test]
    fn parse_token_accepts_exact_form_only() {
        assert_eq!(RefTable::parse_token("[P0]"), Some(0));
        assert_eq!(RefTable::parse_token("  [P3] "), Some(3));
        assert_eq!(RefTable::parse_token("[P]"), None);
        assert_eq!(RefTable::parse_token("[PX]"), None);
        // A real path that merely looks bracket-ish must not be a token.
        assert_eq!(RefTable::parse_token("preference/coding-style"), None);
        assert_eq!(RefTable::parse_token("[P0]/extra"), None);
    }

    #[test]
    fn resolves_append_token_to_canonical_path() {
        let t = table();
        let mut plan = IngestPlan {
            reasoning: String::new(),
            ops: vec![PageOp::Append {
                source_ids: vec![],
                note_path: "[P0]".into(),
                new_facts: vec!["f".into()],
                new_links: vec![],
                new_relations: vec![],
            }],
            schema_proposals: vec![],
        };
        let stats = t.resolve_plan(&mut plan);
        assert_eq!(stats.resolved, 1);
        assert_eq!(stats.dropped_ops, 0);
        match &plan.ops[0] {
            PageOp::Append { note_path, .. } => assert_eq!(note_path, "preference/coding-style"),
            _ => panic!("expected append"),
        }
    }

    #[test]
    fn drops_op_with_out_of_range_target_token() {
        let t = table();
        let mut plan = IngestPlan {
            reasoning: String::new(),
            ops: vec![
                PageOp::Append {
                    source_ids: vec![],
                    note_path: "[P99]".into(), // hallucinated — only P0..P2 exist
                    new_facts: vec!["f".into()],
                    new_links: vec![],
                    new_relations: vec![],
                },
                PageOp::Update {
                    note_path: "[P1]".into(),
                    expected_content_hash: "h".into(),
                    new_facts: vec!["g".into()],
                    reason: "r".into(),
                },
            ],
            schema_proposals: vec![],
        };
        let stats = t.resolve_plan(&mut plan);
        assert_eq!(stats.dropped_ops, 1);
        assert_eq!(plan.ops.len(), 1);
        match &plan.ops[0] {
            PageOp::Update { note_path, .. } => assert_eq!(note_path, "personal/li-wei"),
            _ => panic!("expected the surviving update"),
        }
    }

    #[test]
    fn raw_paths_pass_through_unchanged() {
        // Back-compat: a pre-token planner emits raw paths; nothing is rewritten.
        let t = table();
        let mut plan = IngestPlan {
            reasoning: String::new(),
            ops: vec![PageOp::Append {
                source_ids: vec![],
                note_path: "learning/rust-async".into(),
                new_facts: vec!["f".into()],
                new_links: vec!["learning/tokio".into()],
                new_relations: vec![],
            }],
            schema_proposals: vec![],
        };
        let stats = t.resolve_plan(&mut plan);
        assert_eq!(stats, ResolveStats::default());
        match &plan.ops[0] {
            PageOp::Append {
                note_path,
                new_links,
                ..
            } => {
                assert_eq!(note_path, "learning/rust-async");
                assert_eq!(new_links, &vec!["learning/tokio".to_string()]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn create_links_drop_bad_entries_but_keep_op() {
        let t = table();
        let mut plan = IngestPlan {
            reasoning: String::new(),
            ops: vec![PageOp::Create {
                source_ids: vec![],
                note_path: "learning/new".into(),
                title: "New".into(),
                summary: String::new(),
                facts: vec![],
                links: vec!["[P2]".into(), "[P50]".into(), "learning/raw".into()],
                tags: vec![],
                relations: vec![],
                confidence: 1.0,
                severity: Default::default(),
            }],
            schema_proposals: vec![],
        };
        let stats = t.resolve_plan(&mut plan);
        assert_eq!(stats.resolved, 1);
        assert_eq!(stats.dropped_links, 1);
        assert_eq!(stats.dropped_ops, 0);
        match &plan.ops[0] {
            PageOp::Create {
                note_path, links, ..
            } => {
                assert_eq!(note_path, "learning/new"); // never resolved
                assert_eq!(
                    links,
                    &vec!["projects/aleph".to_string(), "learning/raw".to_string()]
                );
            }
            _ => panic!(),
        }
    }

    #[test]
    fn create_relations_resolve_tokens_and_drop_out_of_range() {
        // The ingest prompt tells the model `"to": "<entity path or [P<n>]
        // token>"`, so relation targets carry the same tokens links do. Before
        // this was wired, `[P2]` reached `Relation.to` verbatim and got written
        // into note frontmatter as a literal `[P2]` edge.
        let t = table();
        let mut plan = IngestPlan {
            reasoning: String::new(),
            ops: vec![PageOp::Create {
                source_ids: vec![],
                note_path: "entity/alice".into(),
                title: "Alice".into(),
                summary: String::new(),
                facts: vec![],
                links: vec![],
                tags: vec![],
                relations: vec![
                    Relation {
                        to: "[P2]".into(),
                        rel_type: "works_at".into(),
                        confidence: 0.9,
                    },
                    Relation {
                        to: "[P50]".into(),
                        rel_type: "hallucinated".into(),
                        confidence: 0.5,
                    },
                    Relation {
                        to: "entity/bob".into(),
                        rel_type: "colleague".into(),
                        confidence: 0.7,
                    },
                ],
                confidence: 1.0,
                severity: Default::default(),
            }],
            schema_proposals: vec![],
        };
        let stats = t.resolve_plan(&mut plan);
        assert_eq!(stats.resolved, 1);
        assert_eq!(stats.dropped_links, 1);
        assert_eq!(stats.dropped_ops, 0);
        match &plan.ops[0] {
            PageOp::Create { relations, .. } => {
                assert_eq!(relations.len(), 2);
                assert_eq!(relations[0].to, "projects/aleph"); // token resolved
                assert_eq!(relations[1].to, "entity/bob"); // raw path passthrough
            }
            _ => panic!(),
        }
    }

    #[test]
    fn link_op_resolves_both_ends_and_mixes_token_with_new_path() {
        let t = table();
        let mut plan = IngestPlan {
            reasoning: String::new(),
            ops: vec![PageOp::Link {
                from: "[P0]".into(),
                to: "learning/freshly-created".into(), // new page from same batch
            }],
            schema_proposals: vec![],
        };
        let stats = t.resolve_plan(&mut plan);
        assert_eq!(stats.resolved, 1);
        match &plan.ops[0] {
            PageOp::Link { from, to } => {
                assert_eq!(from, "preference/coding-style");
                assert_eq!(to, "learning/freshly-created");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn empty_table_passes_everything_through() {
        let t = RefTable::from_related(&[]);
        let mut plan = IngestPlan {
            reasoning: String::new(),
            ops: vec![PageOp::Append {
                source_ids: vec![],
                note_path: "learning/x".into(),
                new_facts: vec!["f".into()],
                new_links: vec![],
                new_relations: vec![],
            }],
            schema_proposals: vec![],
        };
        let stats = t.resolve_plan(&mut plan);
        assert!(t.is_empty());
        assert_eq!(stats, ResolveStats::default());
        assert_eq!(plan.ops.len(), 1);
    }
}
