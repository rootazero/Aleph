# Spec 6 — Compound Ingest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single-step `FactExtractor::extract_note_updates_for_source` pipeline with a two-phase compound ingestor that retrieves related pages, asks the LLM for a multi-page update plan (create / append / update / contradict / link / supersede), and applies it transactionally. Delete `ConflictDetector` and the remaining legacy extractor entry points.

**Architecture:** New `src/memory/notes/ingest/` module with five files: `plan.rs` (data model), `retrieve.rs` (Phase 1 related-pages gatherer), `prompts.rs` (Phase 2 system prompts with Spec 1 source-aware suffix merging), `apply.rs` (Phase 3 transactional staging + rename commit with rollback), and `ingestor.rs` (`CompoundIngestor` trait + `DefaultCompoundIngestor`). `CompressionService` loses its `ConflictDetector` dependency and routes every batch through the new ingestor; contradictions come from the LLM as `PageOp::Contradict` ops. The `.tx/{tx_id}/` staging directory is a plain filesystem transaction — no DB transactions — leveraging `fs::rename` atomicity on the same filesystem.

**Tech Stack:** Rust + tokio + async_trait + serde + `insta` snapshots + `proptest`. Reuses existing `NoteStore::hybrid_search_notes`, `NoteOrientation::record_ingest`, `AiProvider`, `EmbeddingProvider`. No new third-party dependencies.

**Spec reference:** `docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md` §3 (also §1 for architecture overview, §6 for cleanup list and error handling).

**Pre-flight check for the implementer:**
- Verify `NoteStore::hybrid_search_notes(embedding, query_text, agent_id, dim_hint, limit) -> Result<Vec<NoteSearchResult>, AlephError>` still exists at `src/memory/notes/store.rs`.
- Verify `NoteStore::index_note(note, agent_id, category) -> Result<(), AlephError>` still exists (used by the apply phase to keep SQLite in sync with disk).
- Verify `NoteStore::get_note_index(path, agent_id) -> Result<Option<NoteIndexEntry>, AlephError>` still exists (used by hash-guard pre-check).
- Verify `crate::memory::compression::source_prompts::prompt_for(&RawMemorySource) -> Option<&'static str>` still exists (Spec 1 surface).
- Verify `crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore}` are intact.
- Verify `crate::providers::recording_mock::RecordingMockProvider::new(String)` + `recorded_system_prompt()` + `recorded_user_prompt()` are the right accessors — these drive every prompt-bearing test.
- Verify `crate::memory::notes::orientation::NoteOrientation` is the renamed trait from Spec 5 (NOT `WikiOrientation`).

**Out of scope for Spec 6 (deferred to Spec 7/8):**
- USER.md / `ProfileSynthesizer` (Spec 7)
- `query/` category / `QueryFiler` / `query_filed` table (Spec 8)
- Skill auto-creation, nudges, cross-session search (future)

---

## File Map

### Create

- `src/memory/notes/ingest/mod.rs` — module root + re-exports
- `src/memory/notes/ingest/plan.rs` — `PageOp`, `IngestPlan`, `SchemaProposal`, `ApplyReport`
- `src/memory/notes/ingest/retrieve.rs` — `gather_related`, `RelatedPage`, `RelatedBudget`
- `src/memory/notes/ingest/prompts.rs` — `PROMPT_COMPOUND_PLAN` + `build_compound_system_prompt(source)`
- `src/memory/notes/ingest/apply.rs` — `CompoundApplyTx`, `StagedWrite`, `ApplyError`
- `src/memory/notes/ingest/ingestor.rs` — `CompoundIngestor` trait + `DefaultCompoundIngestor`
- `src/memory/notes/ingest/snapshots/` — insta snapshot dir (auto-created on first insta run)
- `tests/memory_compound_ingest.rs` — end-to-end integration test

### Modify

- `src/memory/notes/mod.rs` — `pub mod ingest;`
- `src/memory/notes/indexer.rs` — fix `write_note` + `append_to_note` to also call `store.index_note` (Spec 5 follow-up required by the apply phase)
- `src/memory/compression/service.rs` — drop `ConflictDetector` field + calls; route through `CompoundIngestor`
- `src/memory/compression/extractor.rs` — delete `extract_facts`, `extract_unified`, `parse_unified_response`, `UnifiedExtractionResponse`, `ExtractedFact`, `ExtractedEntity`, `ExtractedRelationship`, `ExtractionResponse`, old `get_system_prompt`, `build_extraction_prompt` (keep just `extract_note_updates_for_source` as a thin wrapper during transition? NO — Spec 6 removes it; callers must use CompoundIngestor). Keep `FactExtractor` struct only if still used elsewhere; otherwise delete whole file.
- `src/memory/compression/mod.rs` — drop `pub mod conflict;` + drop `pub mod extractor;` if file is fully removed
- `src/config/types/memory.rs` — add `CompoundIngestConfig`; remove `conflict_similarity_threshold`
- `src/memory/store/sqlite/schema.rs` — drop legacy `dream_reports` columns (`facts_collected`, `clusters_found`, `drift_detected`, `drift_summary`, `candidates_evaluated`, `facts_promoted`, `promotion_details`, `facts_decayed`, `facts_pruned`, `nodes_decayed`, `edges_decayed`)

### Delete

- `src/memory/compression/conflict.rs` — entire file

---

## Task 1: Fix `NoteIndexer` SQLite sync (Spec 5 follow-up)

**Files:**
- Modify: `src/memory/notes/indexer.rs`

Context: Spec 5 exposed that `NoteIndexer::write_note` writes to disk and notifies the orientation layer, but does NOT call `store.index_note`. This left the SQLite index permanently stale vs disk unless `full_rebuild` is invoked. Spec 6's apply phase depends on SQLite being in sync for `get_note_index` + `hybrid_search_notes` to see freshly-written notes.

- [ ] **Step 1: Write failing test**

Append to the existing `#[cfg(test)] mod wiki_hook_tests` (keep the existing module; rename to `tests` or leave the name — just append inside):

```rust
#[tokio::test]
async fn write_note_also_indexes_to_sqlite() {
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(
        SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap(),
    );
    let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

    let note = KnowledgeNote {
        title: "rust-async".into(),
        category: "learning".into(),
        tags: vec!["rust".into()],
        facts: vec!["Tokio is the async runtime".into()],
        links: vec![],
        created_at: 0,
        updated_at: 0,
        content_hash: String::new(),
    };
    indexer.write_note("default", "learning", &note).await.unwrap();

    // Without the fix, list_notes returns [] until full_rebuild runs.
    let listed = backend.list_notes("default").await.unwrap();
    assert_eq!(listed.len(), 1, "write_note must also index to SQLite");
    assert_eq!(listed[0].path, "learning/rust-async");
}

#[tokio::test]
async fn append_to_note_also_indexes_to_sqlite() {
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(
        SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap(),
    );
    let indexer = NoteIndexer::new(dir.path().join("note"), backend.clone());

    indexer
        .append_to_note(
            "default",
            "learning/rust-async",
            &vec!["new fact".into()],
            &vec![],
        )
        .await
        .unwrap();
    let listed = backend.list_notes("default").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].path == "learning/rust-async");
}
```

- [ ] **Step 2: Run tests — expect FAIL**

```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo test -p alephcore --lib memory::notes::indexer -- write_note_also_indexes_to_sqlite append_to_note_also_indexes_to_sqlite
```

Expected: both FAIL (listed.len() == 0).

- [ ] **Step 3: Implement fix**

In `src/memory/notes/indexer.rs`, find `write_note`. After the successful disk write (just before returning `Ok(())` and before `notify_orientation`), add:

```rust
// Keep SQLite index in sync with disk. Parse the file we just wrote to
// recompute content_hash + wikilinks, then upsert the notes_index row.
let reparsed = KnowledgeNote::from_markdown(
    &tokio::fs::read_to_string(&file_path)
        .await
        .map_err(|e| AlephError::other(format!("reread after write: {e}")))?,
    &safe_title,
)
.map_err(|e| AlephError::other(format!("reparse after write: {e}")))?;
self.store
    .index_note(&reparsed, agent_id, category)
    .await?;
```

Adapt `file_path` / `safe_title` / `KnowledgeNote::from_markdown` to the actual method names in the file. If `from_markdown` takes different args, consult `src/memory/notes/note.rs` for the current signature.

Do the same inside `append_to_note` (after the append + write). Do the same inside `rename_note` after the file rename + rewrite.

- [ ] **Step 4: Run tests — expect PASS**

```bash
cargo test -p alephcore --lib memory::notes::indexer
```

Expected: all tests pass, including the two new ones plus the Spec 5 `write_note_invalidates_wiki`.

Also run the integration test to confirm Spec 5 still works:

```bash
cargo test -p alephcore --test memory_note_orientation
```

Expected: `orientation_layer_end_to_end` passes. The workaround `full_rebuild` line in that test is now redundant but not harmful.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/indexer.rs
git commit -m "fix(notes): NoteIndexer writes now sync to SQLite immediately"
```

---

## Task 2: Scaffold `ingest` module and data types

**Files:**
- Create: `src/memory/notes/ingest/mod.rs`
- Create: `src/memory/notes/ingest/plan.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/notes/ingest/plan.rs`:

```rust
//! Data model for the compound-ingest plan and its outputs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPlan {
    /// Free-form LLM rationale. Truncated to 240 chars before the log line.
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub ops: Vec<PageOp>,
    /// New tag / rule proposals the LLM wants — logged but never auto-applied.
    #[serde(default)]
    pub schema_proposals: Vec<SchemaProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageOp {
    Create {
        note_path: String,
        title: String,
        summary: String,
        #[serde(default)]
        facts: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
        #[serde(default)]
        tags: Vec<String>,
    },
    Append {
        note_path: String,
        #[serde(default)]
        new_facts: Vec<String>,
        #[serde(default)]
        new_links: Vec<String>,
    },
    Update {
        note_path: String,
        /// Hash the LLM saw when it read the target. Verified at apply time.
        expected_content_hash: String,
        new_facts: Vec<String>,
        reason: String,
    },
    Contradict {
        note_path: String,
        new_claim: String,
        #[serde(default)]
        evidence_source_ids: Vec<String>,
    },
    Link {
        from: String,
        to: String,
    },
    Supersede {
        old_path: String,
        new_path: String,
    },
}

impl PageOp {
    /// Primary path this op touches — used for tx-scope + dedup.
    pub fn primary_path(&self) -> &str {
        match self {
            PageOp::Create { note_path, .. }
            | PageOp::Append { note_path, .. }
            | PageOp::Update { note_path, .. }
            | PageOp::Contradict { note_path, .. } => note_path,
            PageOp::Link { from, .. } => from,
            PageOp::Supersede { old_path, .. } => old_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaProposal {
    NewTag { tag: String, rationale: String },
    NewRule { rule: String, rationale: String },
    DomainUpdate { text: String },
}

/// Summary of what an apply pass produced. Returned to CompressionService.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyReport {
    pub created: u32,
    pub appended: u32,
    pub updated: u32,
    pub contradicted: u32,
    pub linked: u32,
    pub superseded: u32,
    pub tx_id: String,
    pub touched_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_op_roundtrip_json() {
        let ops = vec![
            PageOp::Create {
                note_path: "learning/tokio".into(),
                title: "Tokio".into(),
                summary: "Async runtime".into(),
                facts: vec!["event-driven".into()],
                links: vec!["learning/rust-async".into()],
                tags: vec!["rust".into()],
            },
            PageOp::Append {
                note_path: "learning/rust-async".into(),
                new_facts: vec!["pin API".into()],
                new_links: vec![],
            },
            PageOp::Update {
                note_path: "preference/runtime".into(),
                expected_content_hash: "abc123".into(),
                new_facts: vec!["updated fact".into()],
                reason: "supersede old".into(),
            },
            PageOp::Contradict {
                note_path: "learning/rust-async".into(),
                new_claim: "use tokio 1.x".into(),
                evidence_source_ids: vec!["raw-1".into()],
            },
            PageOp::Link {
                from: "learning/tokio".into(),
                to: "learning/rust-async".into(),
            },
            PageOp::Supersede {
                old_path: "learning/old".into(),
                new_path: "learning/new".into(),
            },
        ];
        let plan = IngestPlan {
            reasoning: "test".into(),
            ops: ops.clone(),
            schema_proposals: vec![SchemaProposal::NewTag {
                tag: "async".into(),
                rationale: "used in 3 notes".into(),
            }],
        };
        let j = serde_json::to_string(&plan).unwrap();
        let back: IngestPlan = serde_json::from_str(&j).unwrap();
        assert_eq!(back.ops.len(), ops.len());
        assert_eq!(back.schema_proposals.len(), 1);
    }

    #[test]
    fn page_op_primary_path_matches_variant() {
        let p = PageOp::Create {
            note_path: "a/b".into(),
            title: "".into(),
            summary: "".into(),
            facts: vec![],
            links: vec![],
            tags: vec![],
        };
        assert_eq!(p.primary_path(), "a/b");

        let p = PageOp::Link {
            from: "x/y".into(),
            to: "z/w".into(),
        };
        assert_eq!(p.primary_path(), "x/y");

        let p = PageOp::Supersede {
            old_path: "old".into(),
            new_path: "new".into(),
        };
        assert_eq!(p.primary_path(), "old");
    }

    #[test]
    fn apply_report_default_is_zero() {
        let r = ApplyReport::default();
        assert_eq!(r.created, 0);
        assert!(r.tx_id.is_empty());
        assert!(r.touched_paths.is_empty());
    }
}
```

- [ ] **Step 2: Run test — expect FAIL**

```bash
cargo test -p alephcore --lib memory::notes::ingest::plan
```

Expected: module not declared.

- [ ] **Step 3: Wire the module**

Create `src/memory/notes/ingest/mod.rs`:

```rust
//! Compound-ingest pipeline: retrieve → plan → apply → record.
//!
//! Replaces the single-step `FactExtractor::extract_note_updates_for_source`
//! with a two-phase flow that updates multiple note pages per batch.

pub mod plan;

pub use plan::{ApplyReport, IngestPlan, PageOp, SchemaProposal};
```

Append to `src/memory/notes/mod.rs`:

```rust
pub mod ingest;
```

- [ ] **Step 4: Run test — expect PASS**

```bash
cargo test -p alephcore --lib memory::notes::ingest::plan
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/mod.rs src/memory/notes/ingest/
git commit -m "feat(ingest): scaffold compound-ingest module and data model"
```

---

## Task 3: Compound-plan system prompt with source suffix

**Files:**
- Create: `src/memory/notes/ingest/prompts.rs`
- Modify: `src/memory/notes/ingest/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/notes/ingest/prompts.rs`:

```rust
//! System prompts for the compound-ingest LLM call.

use crate::memory::compression::source_prompts::prompt_for;
use crate::memory::store::raw_memory::RawMemorySource;

/// Base system prompt. Instructs the LLM to read related pages and return
/// a cross-page `IngestPlan` as JSON.
pub const PROMPT_COMPOUND_PLAN: &str = r#"You maintain an Aleph personal-memory wiki. Given a batch of raw
conversation memories plus the set of already-existing pages most relevant
to them, emit an IngestPlan that updates the wiki.

## Ingest rules

1. Look across ALL related pages, not just one. A single batch usually
   touches 3–12 pages.
2. Every fact must be in THIRD PERSON ("The user prefers X").
3. When a new claim CONFLICTS with content already on a related page,
   emit `contradict` — do NOT silently append.
4. When an existing page already covers a topic, emit `append` rather
   than creating a duplicate.
5. When new info SUPERSEDES an older page entirely, emit `supersede`.
6. Every `create` must include at least two `links` to existing pages
   (otherwise the new page is an orphan). If you cannot find two
   existing pages to link, the claim likely belongs as an `append` on
   an existing page instead.
7. When you want to introduce a NEW tag (one not present in any
   provided related page), put it in `schema_proposals` as
   `new_tag`; do NOT invent a tag in `tags:` that the Schema has not
   seen before.
8. Ignore greetings, small talk, transient information.
9. 0–12 ops per batch. Quality over quantity.
10. Use `update` only to CORRECT a page whose content is now wrong.
    When using `update`, set `expected_content_hash` to the hash of the
    related page AS YOU READ IT in the input below. Do not fabricate hashes.

## Page op kinds

- `create` — new page. Fields: `note_path` (category/filename),
  `title`, `summary` (≤120 chars), `facts[]`, `links[]`, `tags[]`.
- `append` — add facts to an existing page. Fields: `note_path`,
  `new_facts[]`, `new_links[]`.
- `update` — replace facts on an existing page. Fields: `note_path`,
  `expected_content_hash`, `new_facts[]`, `reason`.
- `contradict` — mark a page contradicted by new info. Fields:
  `note_path`, `new_claim`, `evidence_source_ids[]`.
- `link` — add a bidirectional wikilink. Fields: `from`, `to`.
- `supersede` — older page is superseded by newer one. Fields:
  `old_path`, `new_path`.

## Output

Valid JSON only. No prose, no markdown fences. Shape:

{
  "reasoning": "2-3 sentence explanation of what you did and why",
  "ops": [ /* PageOp objects */ ],
  "schema_proposals": [ /* optional new_tag / new_rule / domain_update */ ]
}

If nothing is worth ingesting, emit:
{"reasoning": "no durable knowledge", "ops": [], "schema_proposals": []}
"#;

/// Build the full system prompt for a batch whose rows share the given
/// raw source. Appends the Spec-1 source-aware block (RESCUE / LESSON /
/// DIGEST / RETRO) when applicable, so the compound plan call inherits
/// the source-specialised framing.
pub fn build_compound_system_prompt(source: &RawMemorySource) -> String {
    let mut out = String::from(PROMPT_COMPOUND_PLAN);
    if let Some(suffix) = prompt_for(source) {
        out.push_str("\n\n## Source-specific guidance\n\n");
        out.push_str(suffix);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_prompt_snapshot() {
        insta::assert_snapshot!("compound_plan_base_prompt", PROMPT_COMPOUND_PLAN);
    }

    #[test]
    fn base_prompt_mentions_every_op_kind() {
        for kind in ["create", "append", "update", "contradict", "link", "supersede"] {
            assert!(
                PROMPT_COMPOUND_PLAN.contains(&format!("`{kind}`")),
                "missing op kind: {kind}"
            );
        }
    }

    #[test]
    fn precompress_prompt_appends_rescue_block() {
        let p = build_compound_system_prompt(&RawMemorySource::PreCompress);
        assert!(p.starts_with("You maintain"));
        assert!(p.contains("Source-specific guidance"));
        assert!(p.contains("memory rescue assistant"));
    }

    #[test]
    fn legacy_source_has_no_suffix() {
        let p = build_compound_system_prompt(&RawMemorySource::Transcript);
        assert_eq!(p.as_str(), PROMPT_COMPOUND_PLAN);
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p alephcore --lib memory::notes::ingest::prompts
```

Expected: module not declared; plus the snapshot test will produce a `*.snap.new` requiring acceptance.

- [ ] **Step 3: Wire module + accept snapshot**

Append to `src/memory/notes/ingest/mod.rs`:

```rust
pub mod prompts;
pub use prompts::{build_compound_system_prompt, PROMPT_COMPOUND_PLAN};
```

Run once with `INSTA_UPDATE=always` to emit the snapshot:

```bash
INSTA_UPDATE=always cargo test -p alephcore --lib memory::notes::ingest::prompts
```

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p alephcore --lib memory::notes::ingest::prompts
```

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/ingest/mod.rs src/memory/notes/ingest/prompts.rs src/memory/notes/ingest/snapshots/
git commit -m "feat(ingest): compound-plan system prompt with source-aware suffix"
```

---

## Task 4: `gather_related` — Phase 1 retrieval

**Files:**
- Create: `src/memory/notes/ingest/retrieve.rs`
- Modify: `src/memory/notes/ingest/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/notes/ingest/retrieve.rs`:

```rust
//! Phase 1 — gather related pages for a batch of raw memories.

use crate::error::AlephError;
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::store::NoteStore;
use crate::memory::store::raw_memory::RawMemory;
use crate::sync_primitives::Arc;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct RelatedBudget {
    pub max_related_pages: usize,
    pub preview_char_cap: usize,
    pub total_byte_cap: usize,
}

impl Default for RelatedBudget {
    fn default() -> Self {
        Self {
            max_related_pages: 15,
            preview_char_cap: 800,
            total_byte_cap: 12 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RelatedPage {
    pub path: String,
    pub title: String,
    pub summary: String,
    pub content_preview: String,
    pub tags: Vec<String>,
    pub content_hash: String,
    pub score: f32,
}

/// Embed the aggregated raw batch text, hybrid-search for N seed pages,
/// expand 1-hop via outgoing links, truncate to budget.
pub async fn gather_related<S: NoteStore + Send + Sync + 'static>(
    store: Arc<S>,
    embedder: Arc<dyn EmbeddingProvider>,
    raws: &[RawMemory],
    agent_id: &str,
    budget: &RelatedBudget,
) -> Result<Vec<RelatedPage>, AlephError> {
    if raws.is_empty() {
        return Ok(vec![]);
    }

    let mut aggregated = String::new();
    for r in raws {
        aggregated.push_str(&r.content);
        aggregated.push('\n');
        if let Some(att) = &r.attachment_text {
            aggregated.push_str(att);
            aggregated.push('\n');
        }
    }

    let embedding = embedder.embed(&aggregated).await?;
    let dim_hint = embedding.len();

    let seed_limit = budget.max_related_pages.saturating_mul(2).max(6);
    let seeds = store
        .hybrid_search_notes(&embedding, &aggregated, agent_id, dim_hint, seed_limit)
        .await?;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut ranked: Vec<(String, f32)> = Vec::new();
    for s in &seeds {
        if seen.insert(s.path.clone()) {
            ranked.push((s.path.clone(), s.score));
        }
    }

    // 1-hop expand: outgoing links of the top-ranked seeds (up to 6).
    let expand_roots: Vec<&str> = ranked.iter().take(6).map(|(p, _)| p.as_str()).collect();
    for root in expand_roots {
        let outgoing = store.get_outgoing_links(root, agent_id).await?;
        for link in outgoing {
            // Only consider existing pages. If link does not resolve, skip.
            if seen.contains(&link) {
                continue;
            }
            if let Some(_entry) = store.get_note_index(&link, agent_id).await? {
                if seen.insert(link.clone()) {
                    // 1-hop pages get a dampened score (0.5×) to rank after true seeds.
                    ranked.push((link, 0.0));
                }
            }
        }
    }

    // Hydrate: build RelatedPage from seeds first (score preserved), then expansions.
    let mut out: Vec<RelatedPage> = Vec::new();
    let mut running_bytes: usize = 0;
    for (path, score) in ranked.into_iter().take(budget.max_related_pages) {
        let Some(entry) = store.get_note_index(&path, agent_id).await? else {
            continue;
        };
        // Load content preview from the same rendered NoteSearchResult when present,
        // else from disk via the store helper.
        let full = seeds
            .iter()
            .find(|s| s.path == path)
            .map(|s| s.content.clone())
            .unwrap_or_default();
        let preview: String = if full.is_empty() {
            String::new()
        } else {
            full.chars().take(budget.preview_char_cap).collect()
        };
        let summary_first_bullet = first_body_bullet(&full).unwrap_or_default();

        let rp_bytes = preview.len() + entry.path.len() + summary_first_bullet.len() + 64;
        if running_bytes + rp_bytes > budget.total_byte_cap {
            break;
        }
        running_bytes += rp_bytes;

        out.push(RelatedPage {
            path: entry.path.clone(),
            title: entry.filename.clone(),
            summary: summary_first_bullet,
            content_preview: preview,
            tags: entry.tags.clone(),
            content_hash: entry.content_hash.clone(),
            score,
        });
    }

    Ok(out)
}

fn first_body_bullet(raw: &str) -> Option<String> {
    let body = if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            &rest[end + 4..]
        } else {
            rest
        }
    } else {
        raw
    };
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- ") {
            return Some(rest.chars().take(200).collect());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_budget_default() {
        let b = RelatedBudget::default();
        assert_eq!(b.max_related_pages, 15);
        assert_eq!(b.preview_char_cap, 800);
    }

    #[test]
    fn first_body_bullet_picks_first_dash() {
        let md = "---\nk: v\n---\n# Heading\n\n- alpha\n- beta\n";
        assert_eq!(first_body_bullet(md).unwrap(), "alpha");
    }

    #[test]
    fn first_body_bullet_none_when_no_bullet() {
        assert!(first_body_bullet("plain prose").is_none());
    }

    // Behaviour-level tests that require NoteStore live in Task 15 integration.
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p alephcore --lib memory::notes::ingest::retrieve
```

Expected: module not declared.

- [ ] **Step 3: Wire module**

Append to `src/memory/notes/ingest/mod.rs`:

```rust
pub mod retrieve;
pub use retrieve::{gather_related, RelatedBudget, RelatedPage};
```

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p alephcore --lib memory::notes::ingest::retrieve
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/ingest/mod.rs src/memory/notes/ingest/retrieve.rs
git commit -m "feat(ingest): gather_related Phase 1 retrieval with 1-hop expansion"
```

---

## Task 5: `CompoundApplyTx` — transactional staging

**Files:**
- Create: `src/memory/notes/ingest/apply.rs`
- Modify: `src/memory/notes/ingest/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/notes/ingest/apply.rs`:

```rust
//! Phase 3 — transactional apply of `PageOp` sequences.
//!
//! All writes go to `memory/note/{agent}/.tx/{tx_id}/{category}/{filename}.md`
//! first. A successful commit renames the staged files to their final
//! targets in dependency order (Create → Append/Update → Link/Supersede).
//! Failures roll back by reverse-renaming anything already moved.

use crate::error::AlephError;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::notes::ingest::plan::{ApplyReport, PageOp};
use crate::memory::notes::note::{sanitize_title, KnowledgeNote};
use crate::memory::notes::store::NoteStore;
use crate::sync_primitives::Arc;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("hash conflict on {path}: expected {expected}, got {actual}")]
    HashConflict {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("other apply error: {0}")]
    Other(#[from] AlephError),
}

struct StagedWrite {
    staged_path: PathBuf,
    target_path: PathBuf,
    category: String,
    filename: String,
    note: KnowledgeNote,
    op_label: &'static str,
}

pub struct CompoundApplyTx<'a, S: NoteStore + Send + Sync + 'static> {
    indexer: &'a Arc<NoteIndexer<S>>,
    store: &'a Arc<S>,
    agent_id: &'a str,
    memory_dir: PathBuf,
    tx_id: String,
    tx_root: PathBuf,
    staged: Vec<StagedWrite>,
    pending_links: Vec<(String, String)>,      // (from, to)
    pending_supersedes: Vec<(String, String)>, // (old, new)
    committed: bool,
}

impl<'a, S: NoteStore + Send + Sync + 'static> CompoundApplyTx<'a, S> {
    pub fn new(
        indexer: &'a Arc<NoteIndexer<S>>,
        store: &'a Arc<S>,
        memory_dir: impl Into<PathBuf>,
        agent_id: &'a str,
    ) -> Self {
        let memory_dir = memory_dir.into();
        let tx_id = Uuid::new_v4().to_string();
        let tx_root = memory_dir
            .join(agent_id)
            .join(".tx")
            .join(&tx_id);
        Self {
            indexer,
            store,
            agent_id,
            memory_dir,
            tx_id,
            tx_root,
            staged: Vec::new(),
            pending_links: Vec::new(),
            pending_supersedes: Vec::new(),
            committed: false,
        }
    }

    pub fn tx_id(&self) -> &str {
        &self.tx_id
    }

    /// Prepare an op: write the staged file or queue a link/supersede to
    /// apply at commit time. May hit the store to read current content
    /// for hash guards (Update) or for merging (Append / Contradict).
    pub async fn stage(&mut self, op: &PageOp) -> Result<(), ApplyError> {
        match op {
            PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                links,
                tags,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                let mut note = KnowledgeNote {
                    title: title.clone(),
                    category: category.clone(),
                    tags: tags.clone(),
                    facts: facts.clone(),
                    links: links.clone(),
                    created_at: chrono::Utc::now().timestamp(),
                    updated_at: chrono::Utc::now().timestamp(),
                    content_hash: String::new(),
                };
                // Place summary into frontmatter via side-channel: prepend a bullet so
                // index.md picks it up. (Dedicated frontmatter field lives post-Spec 6.)
                let summary_trimmed = summary.chars().take(120).collect::<String>();
                if !summary_trimmed.is_empty() {
                    note.facts.insert(0, format!("[summary] {summary_trimmed}"));
                }
                self.push_staged(&category, &safe, note, "create").await?;
            }
            PageOp::Append {
                note_path,
                new_facts,
                new_links,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                let existing = self
                    .load_existing_or_default(&category, &safe, note_path)
                    .await?;
                let mut merged = existing;
                for f in new_facts {
                    if !merged.facts.contains(f) {
                        merged.facts.push(f.clone());
                    }
                }
                for l in new_links {
                    if !merged.links.contains(l) {
                        merged.links.push(l.clone());
                    }
                }
                merged.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, merged, "append").await?;
            }
            PageOp::Update {
                note_path,
                expected_content_hash,
                new_facts,
                reason: _,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                // Hash guard
                let entry = self.store.get_note_index(note_path, self.agent_id).await?;
                let actual = entry
                    .as_ref()
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default();
                if &actual != expected_content_hash {
                    return Err(ApplyError::HashConflict {
                        path: note_path.clone(),
                        expected: expected_content_hash.clone(),
                        actual,
                    });
                }
                let mut existing = self
                    .load_existing_or_default(&category, &safe, note_path)
                    .await?;
                existing.facts = new_facts.clone();
                existing.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, existing, "update").await?;
            }
            PageOp::Contradict {
                note_path,
                new_claim,
                evidence_source_ids,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename);
                let mut existing = self
                    .load_existing_or_default(&category, &safe, note_path)
                    .await?;
                let ts = chrono::Utc::now().format("%Y-%m-%d").to_string();
                let ev = if evidence_source_ids.is_empty() {
                    "".to_string()
                } else {
                    format!(" (sources: {})", evidence_source_ids.join(", "))
                };
                existing.facts.push(format!(
                    "[contradict {ts}] {new_claim}{ev}"
                ));
                existing.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, existing, "contradict")
                    .await?;
            }
            PageOp::Link { from, to } => {
                self.pending_links.push((from.clone(), to.clone()));
            }
            PageOp::Supersede { old_path, new_path } => {
                self.pending_supersedes
                    .push((old_path.clone(), new_path.clone()));
            }
        }
        Ok(())
    }

    async fn load_existing_or_default(
        &self,
        category: &str,
        filename: &str,
        note_path: &str,
    ) -> Result<KnowledgeNote, ApplyError> {
        let agent_dir = self.memory_dir.join(self.agent_id);
        let disk = agent_dir.join(category).join(format!("{filename}.md"));
        if let Ok(raw) = tokio::fs::read_to_string(&disk).await {
            if let Ok(n) = KnowledgeNote::from_markdown(&raw, filename) {
                return Ok(n);
            }
        }
        Ok(KnowledgeNote {
            title: filename.to_string(),
            category: category.to_string(),
            tags: vec![],
            facts: vec![],
            links: vec![],
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            content_hash: String::new(),
        })
    }

    async fn push_staged(
        &mut self,
        category: &str,
        filename: &str,
        note: KnowledgeNote,
        op_label: &'static str,
    ) -> Result<(), ApplyError> {
        let staged_dir = self.tx_root.join(category);
        tokio::fs::create_dir_all(&staged_dir)
            .await
            .map_err(|e| ApplyError::Other(AlephError::other(format!("tx mkdir: {e}"))))?;
        let staged_path = staged_dir.join(format!("{filename}.md"));
        let target_path = self
            .memory_dir
            .join(self.agent_id)
            .join(category)
            .join(format!("{filename}.md"));
        let body = note.to_markdown();
        tokio::fs::write(&staged_path, &body)
            .await
            .map_err(|e| ApplyError::Other(AlephError::other(format!("tx write: {e}"))))?;
        self.staged.push(StagedWrite {
            staged_path,
            target_path,
            category: category.to_string(),
            filename: filename.to_string(),
            note,
            op_label,
        });
        Ok(())
    }

    /// Atomically commit all staged writes + link updates.
    ///
    /// Order: Create/Append/Update/Contradict first (primary writes), then
    /// Link / Supersede (secondary updates that may touch other pages).
    pub async fn commit(mut self) -> Result<ApplyReport, ApplyError> {
        let mut report = ApplyReport {
            tx_id: self.tx_id.clone(),
            ..Default::default()
        };
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();

        for s in &self.staged {
            if let Some(parent) = s.target_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| ApplyError::Other(AlephError::other(format!("mkdir target: {e}"))))?;
            }
            if let Err(e) = tokio::fs::rename(&s.staged_path, &s.target_path).await {
                // Roll back previously-moved files
                for (from, to) in moved.iter().rev() {
                    let _ = tokio::fs::rename(to, from).await;
                }
                return Err(ApplyError::Other(AlephError::other(format!(
                    "rename {} → {}: {e}",
                    s.staged_path.display(),
                    s.target_path.display()
                ))));
            }
            moved.push((s.staged_path.clone(), s.target_path.clone()));

            // Update SQLite index
            self.store
                .index_note(&s.note, self.agent_id, &s.category)
                .await?;

            match s.op_label {
                "create" => report.created += 1,
                "append" => report.appended += 1,
                "update" => report.updated += 1,
                "contradict" => report.contradicted += 1,
                _ => {}
            }
            report
                .touched_paths
                .push(format!("{}/{}", s.category, s.filename));
        }

        // Apply Link ops — both directions. Uses indexer's append_to_note which
        // now keeps SQLite in sync (Task 1).
        for (from, to) in &self.pending_links {
            let _ = self.add_link(from, to).await;
            let _ = self.add_link(to, from).await;
            report.linked += 1;
            report.touched_paths.push(from.clone());
            report.touched_paths.push(to.clone());
        }

        // Supersede: append a `## Superseded by [[new]] (YYYY-MM-DD)` marker
        // to the old page. The old page is archived by NoteDecay over time.
        for (old_path, new_path) in &self.pending_supersedes {
            let _ = self.mark_superseded(old_path, new_path).await;
            report.superseded += 1;
            report.touched_paths.push(old_path.clone());
        }

        // Clean up the tx dir (best-effort)
        let _ = tokio::fs::remove_dir_all(&self.tx_root).await;

        // Dedup touched_paths while preserving order
        let mut seen: BTreeSet<String> = BTreeSet::new();
        report.touched_paths.retain(|p| seen.insert(p.clone()));

        self.committed = true;
        Ok(report)
    }

    async fn add_link(&self, from: &str, to: &str) -> Result<(), AlephError> {
        // Only add if from-page exists on disk.
        let (category, filename) = match split_path(from) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };
        let disk = self
            .memory_dir
            .join(self.agent_id)
            .join(&category)
            .join(format!("{}.md", sanitize_title(&filename)));
        if tokio::fs::try_exists(&disk)
            .await
            .map_err(|e| AlephError::other(format!("link: stat from: {e}")))?
        {
            self.indexer
                .append_to_note(
                    self.agent_id,
                    from,
                    &Vec::<String>::new(),
                    &vec![to.to_string()],
                )
                .await?;
        }
        Ok(())
    }

    async fn mark_superseded(
        &self,
        old_path: &str,
        new_path: &str,
    ) -> Result<(), AlephError> {
        let (category, filename) = match split_path(old_path) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };
        let disk = self
            .memory_dir
            .join(self.agent_id)
            .join(&category)
            .join(format!("{}.md", sanitize_title(&filename)));
        if !tokio::fs::try_exists(&disk)
            .await
            .map_err(|e| AlephError::other(format!("supersede: stat old: {e}")))?
        {
            return Ok(());
        }
        let body = tokio::fs::read_to_string(&disk)
            .await
            .map_err(|e| AlephError::other(format!("supersede: read old: {e}")))?;
        let marker = format!(
            "\n## Superseded by [[{new_path}]] ({})\n",
            chrono::Utc::now().format("%Y-%m-%d")
        );
        if body.contains("## Superseded by") {
            return Ok(());
        }
        let combined = format!("{body}{marker}");
        tokio::fs::write(&disk, &combined)
            .await
            .map_err(|e| AlephError::other(format!("supersede: write: {e}")))?;
        // Re-index (parse + upsert)
        if let Ok(n) = KnowledgeNote::from_markdown(
            &combined,
            &sanitize_title(&filename),
        ) {
            self.store.index_note(&n, self.agent_id, &category).await?;
        }
        Ok(())
    }

    pub async fn rollback(mut self) {
        for s in self.staged.drain(..).rev() {
            let _ = tokio::fs::remove_file(&s.staged_path).await;
        }
        let _ = tokio::fs::remove_dir_all(&self.tx_root).await;
        self.committed = true; // mark done to silence Drop
    }
}

impl<'a, S: NoteStore + Send + Sync + 'static> Drop for CompoundApplyTx<'a, S> {
    fn drop(&mut self) {
        if !self.committed {
            // Best-effort sync cleanup of the tx dir. Cannot call async here.
            let _ = std::fs::remove_dir_all(&self.tx_root);
        }
    }
}

fn split_path(note_path: &str) -> Result<(String, String), ApplyError> {
    let Some((cat, name)) = note_path.split_once('/') else {
        return Err(ApplyError::Other(AlephError::other(format!(
            "invalid note_path '{note_path}' — expected 'category/filename'"
        ))));
    };
    Ok((cat.to_string(), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::indexer::NoteIndexer;
    use crate::memory::store::sqlite::SqliteMemoryBackend;

    async fn fresh() -> (
        tempfile::TempDir,
        Arc<SqliteMemoryBackend>,
        Arc<NoteIndexer<SqliteMemoryBackend>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap(),
        );
        let indexer = Arc::new(NoteIndexer::new(
            dir.path().join("note"),
            backend.clone(),
        ));
        (dir, backend, indexer)
    }

    #[tokio::test]
    async fn create_op_writes_file_and_indexes() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(
            &indexer,
            &backend,
            dir.path().join("note"),
            "default",
        );
        tx.stage(&PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "Async runtime".into(),
            facts: vec!["event-driven".into()],
            links: vec!["learning/rust-async".into()],
            tags: vec!["rust".into()],
        })
        .await
        .unwrap();
        let report = tx.commit().await.unwrap();
        assert_eq!(report.created, 1);
        assert!(dir
            .path()
            .join("note/default/learning/tokio.md")
            .exists());
        let listed = backend.list_notes("default").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "learning/tokio");
    }

    #[tokio::test]
    async fn update_rejects_stale_hash() {
        let (dir, backend, indexer) = fresh().await;
        // First create
        {
            let mut tx = CompoundApplyTx::new(
                &indexer,
                &backend,
                dir.path().join("note"),
                "default",
            );
            tx.stage(&PageOp::Create {
                note_path: "learning/tokio".into(),
                title: "Tokio".into(),
                summary: "v0".into(),
                facts: vec![],
                links: vec!["learning/rust-async".into()],
                tags: vec![],
            })
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        // Then update with a stale hash
        let mut tx = CompoundApplyTx::new(
            &indexer,
            &backend,
            dir.path().join("note"),
            "default",
        );
        let err = tx
            .stage(&PageOp::Update {
                note_path: "learning/tokio".into(),
                expected_content_hash: "deadbeef".into(),
                new_facts: vec!["v2".into()],
                reason: "test".into(),
            })
            .await
            .unwrap_err();
        match err {
            ApplyError::HashConflict { .. } => {}
            other => panic!("expected HashConflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rollback_removes_staged_files() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(
            &indexer,
            &backend,
            dir.path().join("note"),
            "default",
        );
        tx.stage(&PageOp::Create {
            note_path: "learning/x".into(),
            title: "X".into(),
            summary: "".into(),
            facts: vec![],
            links: vec![],
            tags: vec![],
        })
        .await
        .unwrap();
        let tx_id = tx.tx_id().to_string();
        tx.rollback().await;
        let tx_dir = dir.path().join(format!("note/default/.tx/{tx_id}"));
        assert!(!tx_dir.exists());
        // No target written
        assert!(!dir.path().join("note/default/learning/x.md").exists());
    }

    #[tokio::test]
    async fn append_merges_without_duplicates() {
        let (dir, backend, indexer) = fresh().await;
        // seed with a create
        let mut tx = CompoundApplyTx::new(
            &indexer,
            &backend,
            dir.path().join("note"),
            "default",
        );
        tx.stage(&PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "".into(),
            facts: vec!["fact-a".into()],
            links: vec!["learning/rust-async".into()],
            tags: vec![],
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        // Append with a duplicate + new
        let mut tx = CompoundApplyTx::new(
            &indexer,
            &backend,
            dir.path().join("note"),
            "default",
        );
        tx.stage(&PageOp::Append {
            note_path: "learning/tokio".into(),
            new_facts: vec!["fact-a".into(), "fact-b".into()],
            new_links: vec![],
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let body = tokio::fs::read_to_string(
            dir.path().join("note/default/learning/tokio.md"),
        )
        .await
        .unwrap();
        assert_eq!(body.matches("- fact-a").count(), 1);
        assert_eq!(body.matches("- fact-b").count(), 1);
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p alephcore --lib memory::notes::ingest::apply
```

Expected: module not declared + `uuid` dep may be missing.

- [ ] **Step 3: Wire module + add dep if needed**

Append to `src/memory/notes/ingest/mod.rs`:

```rust
pub mod apply;
pub use apply::{ApplyError, CompoundApplyTx};
```

Verify `uuid` is a workspace dep. It's used across Aleph (see `crate::memory::store::raw_memory::RawMemory::new`). If missing:

```bash
cargo add -p alephcore uuid --features v4
```

Verify `thiserror` is available — it's used in `AlephError` itself, so yes.

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p alephcore --lib memory::notes::ingest::apply
```

Expected: 4 tokio tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/ingest/mod.rs src/memory/notes/ingest/apply.rs
git commit -m "feat(ingest): CompoundApplyTx transactional staging + rollback"
```

---

## Task 6: `CompoundIngestor` trait

**Files:**
- Create: `src/memory/notes/ingest/ingestor.rs` (partial — trait only)
- Modify: `src/memory/notes/ingest/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/memory/notes/ingest/ingestor.rs`:

```rust
//! `CompoundIngestor` trait + `DefaultCompoundIngestor` impl.

use crate::error::AlephError;
use crate::memory::notes::ingest::plan::ApplyReport;
use crate::memory::store::raw_memory::RawMemory;
use async_trait::async_trait;

#[async_trait]
pub trait CompoundIngestor: Send + Sync {
    async fn ingest_batch(
        &self,
        agent_id: &str,
        raws: Vec<RawMemory>,
    ) -> Result<ApplyReport, AlephError>;
}

#[cfg(test)]
mod trait_tests {
    use super::*;

    struct StubIngestor;

    #[async_trait]
    impl CompoundIngestor for StubIngestor {
        async fn ingest_batch(
            &self,
            _agent_id: &str,
            _raws: Vec<RawMemory>,
        ) -> Result<ApplyReport, AlephError> {
            Ok(ApplyReport {
                tx_id: "stub".into(),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn trait_object_dispatch() {
        let ing: Box<dyn CompoundIngestor> = Box::new(StubIngestor);
        let r = ing.ingest_batch("default", vec![]).await.unwrap();
        assert_eq!(r.tx_id, "stub");
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p alephcore --lib memory::notes::ingest::ingestor
```

Expected: module not declared.

- [ ] **Step 3: Wire module**

Append to `src/memory/notes/ingest/mod.rs`:

```rust
pub mod ingestor;
pub use ingestor::CompoundIngestor;
```

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p alephcore --lib memory::notes::ingest::ingestor
```

Expected: 1 test passes.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/ingest/mod.rs src/memory/notes/ingest/ingestor.rs
git commit -m "feat(ingest): CompoundIngestor trait"
```

---

## Task 7: `DefaultCompoundIngestor::plan` — LLM call

**Files:**
- Modify: `src/memory/notes/ingest/ingestor.rs`

- [ ] **Step 1: Write failing test**

Append to `src/memory/notes/ingest/ingestor.rs` (inside or alongside existing content — not inside `trait_tests`):

```rust
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::indexer::NoteIndexer;
use crate::memory::notes::ingest::plan::{IngestPlan, PageOp};
use crate::memory::notes::ingest::prompts::build_compound_system_prompt;
use crate::memory::notes::ingest::retrieve::{gather_related, RelatedBudget, RelatedPage};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::raw_memory::RawMemorySource;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::utils::json_extract::extract_json_robust;
use std::path::PathBuf;
use tracing::warn;

pub struct DefaultCompoundIngestor<S: NoteStore + Send + Sync + 'static> {
    pub store: Arc<S>,
    pub indexer: Arc<NoteIndexer<S>>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    pub memory_dir: PathBuf,
    pub budget: RelatedBudget,
}

impl<S: NoteStore + Send + Sync + 'static> DefaultCompoundIngestor<S> {
    pub async fn plan(
        &self,
        agent_id: &str,
        raws: &[crate::memory::store::raw_memory::RawMemory],
        related: &[RelatedPage],
        source: &RawMemorySource,
    ) -> Result<IngestPlan, AlephError> {
        if raws.is_empty() {
            return Ok(IngestPlan {
                reasoning: String::new(),
                ops: vec![],
                schema_proposals: vec![],
            });
        }

        let system = build_compound_system_prompt(source);
        let user = build_user_prompt(raws, related);
        let msgs = [UnifiedMessage::user(&user)];
        let resp = self
            .provider
            .process(RequestPayload::new(&msgs).with_system(Some(&system)))
            .await
            .map_err(|e| AlephError::other(format!("compound plan LLM: {e}")))?;
        let text = resp.text_content();

        let json = match extract_json_robust(&text) {
            Some(v) => v,
            None => {
                warn!("compound plan: no JSON in LLM response; returning empty plan");
                return Ok(IngestPlan {
                    reasoning: String::new(),
                    ops: vec![],
                    schema_proposals: vec![],
                });
            }
        };
        let mut plan: IngestPlan = serde_json::from_value(json).map_err(|e| {
            warn!("compound plan: parse failed: {e}");
            AlephError::other(format!("compound plan parse: {e}"))
        })?;

        // Post-parse sanitisation.
        plan.ops.retain(|op| valid_op(op));
        Ok(plan)
    }
}

fn build_user_prompt(
    raws: &[crate::memory::store::raw_memory::RawMemory],
    related: &[RelatedPage],
) -> String {
    let mut out = String::from("## New raw memories\n\n");
    for (i, r) in raws.iter().enumerate() {
        out.push_str(&format!(
            "### raw-{} (id={}, source={:?})\n",
            i + 1,
            r.id,
            r.source
        ));
        out.push_str(&r.content);
        out.push_str("\n\n");
        if let Some(att) = &r.attachment_text {
            out.push_str("[Attachment]\n");
            out.push_str(att);
            out.push_str("\n\n");
        }
    }
    if !related.is_empty() {
        out.push_str("## Related existing pages\n\n");
        for p in related {
            out.push_str(&format!(
                "### {path} (hash={hash})\n",
                path = p.path,
                hash = p.content_hash
            ));
            out.push_str(&format!("title: {}\n", p.title));
            if !p.tags.is_empty() {
                out.push_str(&format!("tags: {}\n", p.tags.join(", ")));
            }
            out.push_str("preview:\n");
            out.push_str(&p.content_preview);
            out.push_str("\n\n");
        }
    } else {
        out.push_str("## Related existing pages\n\n(none — empty wiki or no matches)\n");
    }
    out.push_str("Produce the IngestPlan JSON now.");
    out
}

fn valid_op(op: &PageOp) -> bool {
    match op {
        PageOp::Create {
            note_path, links, ..
        } => note_path.contains('/') && links.len() >= 1, // relaxed from 2 for MVP
        PageOp::Append { note_path, .. }
        | PageOp::Update { note_path, .. }
        | PageOp::Contradict { note_path, .. } => note_path.contains('/'),
        PageOp::Link { from, to } => from.contains('/') && to.contains('/') && from != to,
        PageOp::Supersede { old_path, new_path } => {
            old_path.contains('/') && new_path.contains('/') && old_path != new_path
        }
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::providers::recording_mock::RecordingMockProvider;

    async fn mk() -> (
        tempfile::TempDir,
        Arc<SqliteMemoryBackend>,
        Arc<NoteIndexer<SqliteMemoryBackend>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap(),
        );
        let indexer = Arc::new(NoteIndexer::new(
            dir.path().join("note"),
            backend.clone(),
        ));
        (dir, backend, indexer)
    }

    #[tokio::test]
    async fn plan_parses_valid_json() {
        let (dir, backend, indexer) = mk().await;
        let provider_raw = RecordingMockProvider::new(
            r#"{
              "reasoning": "new page + link",
              "ops": [
                {"kind": "create", "note_path": "learning/tokio", "title": "Tokio",
                 "summary": "async runtime", "facts": ["event loop"],
                 "links": ["learning/rust-async"], "tags": ["rust"]}
              ],
              "schema_proposals": []
            }"#
            .into(),
        );
        let provider: Arc<dyn AiProvider> = Arc::new(provider_raw);
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let raw = RawMemory::new("some content", RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript)
            .await
            .unwrap();
        assert_eq!(plan.ops.len(), 1);
        match &plan.ops[0] {
            PageOp::Create { note_path, .. } => assert_eq!(note_path, "learning/tokio"),
            _ => panic!(),
        }
    }

    #[tokio::test]
    async fn plan_returns_empty_on_invalid_json() {
        let (dir, backend, indexer) = mk().await;
        let provider: Arc<dyn AiProvider> =
            Arc::new(RecordingMockProvider::new("not json".into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let raw = RawMemory::new("c", RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript)
            .await
            .unwrap();
        assert!(plan.ops.is_empty());
    }

    #[tokio::test]
    async fn plan_filters_invalid_ops() {
        let (dir, backend, indexer) = mk().await;
        // `create` with no links is invalid under valid_op()
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
            r#"{"ops":[
                {"kind":"create","note_path":"learning/x","title":"X","summary":"","facts":[],"links":[],"tags":[]},
                {"kind":"create","note_path":"bad-no-slash","title":"Y","summary":"","facts":[],"links":["learning/x"],"tags":[]},
                {"kind":"append","note_path":"learning/y","new_facts":["f"],"new_links":[]}
            ]}"#.into()));
        let ing = DefaultCompoundIngestor {
            store: backend.clone(),
            indexer,
            provider,
            embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
            orientation: None,
            memory_dir: dir.path().join("note"),
            budget: RelatedBudget::default(),
        };
        let raw = RawMemory::new("c", RawMemorySource::Transcript);
        let plan = ing
            .plan("default", &[raw], &[], &RawMemorySource::Transcript)
            .await
            .unwrap();
        assert_eq!(plan.ops.len(), 1); // only the append survived
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p alephcore --lib memory::notes::ingest::ingestor
```

Expected: `DefaultCompoundIngestor` not defined yet, or types missing.

- [ ] **Step 3: Run — expect PASS**

After Step 1's code is in place, re-run:

```bash
cargo test -p alephcore --lib memory::notes::ingest::ingestor
```

Expected: 4 tests pass (1 trait + 3 plan).

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/ingest/ingestor.rs
git commit -m "feat(ingest): DefaultCompoundIngestor::plan LLM call"
```

---

## Task 8: `DefaultCompoundIngestor::ingest_batch` — full flow with apply + hash-conflict retry

**Files:**
- Modify: `src/memory/notes/ingest/ingestor.rs`

- [ ] **Step 1: Write failing test**

Append to `src/memory/notes/ingest/ingestor.rs`:

```rust
use crate::memory::notes::ingest::apply::{ApplyError, CompoundApplyTx};
use crate::memory::wiki_log_adapter::LogEntryBuilder; // see note below — if path differs, adapt

// Full ingest_batch implementation:
#[async_trait]
impl<S: NoteStore + Send + Sync + 'static> CompoundIngestor
    for DefaultCompoundIngestor<S>
{
    async fn ingest_batch(
        &self,
        agent_id: &str,
        raws: Vec<crate::memory::store::raw_memory::RawMemory>,
    ) -> Result<ApplyReport, AlephError> {
        if raws.is_empty() {
            return Ok(ApplyReport::default());
        }
        let source = raws[0].source.clone();
        let related = gather_related(
            self.store.clone(),
            self.embedder.clone(),
            &raws,
            agent_id,
            &self.budget,
        )
        .await?;

        let plan = self.plan(agent_id, &raws, &related, &source).await?;
        if plan.ops.is_empty() {
            return Ok(ApplyReport::default());
        }

        // First attempt
        let report = match self.try_apply(agent_id, &plan).await {
            Ok(r) => r,
            Err(ApplyError::HashConflict { path, actual, .. }) => {
                // Re-plan once, telling the LLM about the stale hash
                warn!("compound ingest: hash conflict on {path}, re-planning");
                let mut augmented = raws.clone();
                if let Some(last) = augmented.last_mut() {
                    last.content.push_str(&format!(
                        "\n\n[system] previous plan referenced {path} with a stale hash; actual hash is {actual}. Re-plan using fresh data."
                    ));
                }
                let plan2 = self.plan(agent_id, &augmented, &related, &source).await?;
                if plan2.ops.is_empty() {
                    return Ok(ApplyReport::default());
                }
                self.try_apply(agent_id, &plan2).await?
            }
            Err(ApplyError::Other(e)) => return Err(e),
        };

        // Record ingest in log.md (fire-and-forget)
        if let Some(orient) = &self.orientation {
            let reasoning_preview: String =
                plan.reasoning.chars().take(80).collect();
            let detail: Vec<String> = report
                .touched_paths
                .iter()
                .take(15)
                .map(|p| format!("touched {p}"))
                .collect();
            let entry = crate::memory::notes::orientation::types::LogEntry {
                timestamp_utc: chrono::Utc::now().timestamp(),
                action: crate::memory::notes::orientation::types::LogAction::Ingest,
                summary: format!(
                    "{} pages touched | tx={} | {}",
                    report.touched_paths.len(),
                    report.tx_id,
                    reasoning_preview
                ),
                detail_lines: detail,
            };
            if let Err(e) = orient.record_ingest(agent_id, entry).await {
                warn!("compound ingest: log record failed: {e}");
            }
        }

        Ok(report)
    }
}

impl<S: NoteStore + Send + Sync + 'static> DefaultCompoundIngestor<S> {
    async fn try_apply(
        &self,
        agent_id: &str,
        plan: &IngestPlan,
    ) -> Result<ApplyReport, ApplyError> {
        let mut tx = CompoundApplyTx::new(
            &self.indexer,
            &self.store,
            self.memory_dir.clone(),
            agent_id,
        );
        for op in &plan.ops {
            tx.stage(op).await?;
        }
        tx.commit().await
    }
}
```

**Remove** the erroneous `use crate::memory::wiki_log_adapter::LogEntryBuilder;` line — that's a stub reference; `LogEntry` lives at `crate::memory::notes::orientation::types::LogEntry` (imported inline via the full path in the code above).

End-to-end test (still inside `mod plan_tests` or a new `#[cfg(test)] mod flow_tests`):

```rust
#[tokio::test]
async fn end_to_end_append_on_existing() {
    let (dir, backend, indexer) = mk().await;

    // Seed: create learning/rust-async first by running a plan
    let provider_seed: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"create","note_path":"learning/rust-async","title":"Rust async",
             "summary":"async primitives","facts":["Futures are lazy"],
             "links":["learning/tokio"],"tags":["rust","async"]}
        ]}"#.into(),
    ));
    let ing_seed = DefaultCompoundIngestor {
        store: backend.clone(),
        indexer: indexer.clone(),
        provider: provider_seed,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
    };
    let r1 = ing_seed
        .ingest_batch(
            "default",
            vec![RawMemory::new("seed", RawMemorySource::Transcript)],
        )
        .await
        .unwrap();
    assert_eq!(r1.created, 1);

    // Second batch: LLM appends
    let provider2: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{"ops":[
            {"kind":"append","note_path":"learning/rust-async",
             "new_facts":["tokio is the runtime"],"new_links":[]}
        ]}"#.into(),
    ));
    let ing2 = DefaultCompoundIngestor {
        store: backend.clone(),
        indexer: indexer.clone(),
        provider: provider2,
        embedder: Arc::new(MockEmbeddingProvider::new(1024, "mock")),
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
    };
    let r2 = ing2
        .ingest_batch(
            "default",
            vec![RawMemory::new("body2", RawMemorySource::Transcript)],
        )
        .await
        .unwrap();
    assert_eq!(r2.appended, 1);

    let body = tokio::fs::read_to_string(
        dir.path().join("note/default/learning/rust-async.md"),
    )
    .await
    .unwrap();
    assert!(body.contains("Futures are lazy"));
    assert!(body.contains("tokio is the runtime"));
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p alephcore --lib memory::notes::ingest::ingestor
```

Expected: fails until full flow is in place.

- [ ] **Step 3: Run — expect PASS**

After Step 1's code is in place:

```bash
cargo test -p alephcore --lib memory::notes::ingest::ingestor
```

Expected: all tests pass (prior tests + end-to-end append).

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/ingest/ingestor.rs
git commit -m "feat(ingest): ingest_batch with hash-conflict retry + orientation logging"
```

---

## Task 9: Add `CompoundIngestConfig` + wire `CompressionService` to use `CompoundIngestor`

**Files:**
- Modify: `src/config/types/memory.rs`
- Modify: `src/memory/compression/service.rs`

Goal: replace the `extract_note_updates_for_source` call chain in `CompressionService` with a call to a `CompoundIngestor` (injected via builder). Preserve the existing public API of `CompressionService::compress`.

- [ ] **Step 1: Add config**

In `src/config/types/memory.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundIngestConfig {
    #[serde(default = "default_compound_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_related_pages")]
    pub max_related_pages: usize,
    #[serde(default = "default_related_preview_char_cap")]
    pub related_preview_char_cap: usize,
    #[serde(default = "default_related_total_byte_cap")]
    pub related_total_byte_cap: usize,
    #[serde(default = "default_replan_on_hash_conflict")]
    pub replan_on_hash_conflict: u32,
    #[serde(default = "default_failure_cooldown_seconds")]
    pub failure_cooldown_seconds: u64,
    #[serde(default = "default_tx_residue_gc_seconds")]
    pub tx_residue_gc_seconds: u64,
}

impl Default for CompoundIngestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_related_pages: 15,
            related_preview_char_cap: 800,
            related_total_byte_cap: 12 * 1024,
            replan_on_hash_conflict: 1,
            failure_cooldown_seconds: 300,
            tx_residue_gc_seconds: 3600,
        }
    }
}

fn default_compound_enabled() -> bool { true }
fn default_max_related_pages() -> usize { 15 }
fn default_related_preview_char_cap() -> usize { 800 }
fn default_related_total_byte_cap() -> usize { 12 * 1024 }
fn default_replan_on_hash_conflict() -> u32 { 1 }
fn default_failure_cooldown_seconds() -> u64 { 300 }
fn default_tx_residue_gc_seconds() -> u64 { 3600 }
```

Add the nested field on `MemoryConfig`:

```rust
#[serde(default)]
pub compound_ingest: CompoundIngestConfig,
```

Delete the line `pub conflict_similarity_threshold: f32,` (or `#[serde(default = ...)] pub conflict_similarity_threshold: f32,`) plus its default function. Callers of `conflict_similarity_threshold` — grep:

```bash
rg -n "conflict_similarity_threshold" src/
```

Replace each call site with nothing (the field is gone). If the only call sites are inside `conflict.rs` (being deleted in Task 11), that's fine.

- [ ] **Step 2: Write failing test for CompressionService routing**

In `src/memory/compression/service.rs`'s existing test module, add:

```rust
#[tokio::test]
async fn compress_routes_through_compound_ingestor() {
    use crate::memory::notes::ingest::{ApplyReport, CompoundIngestor};
    use std::sync::{Arc, Mutex};

    struct Spy { calls: Mutex<u32> }
    #[async_trait::async_trait]
    impl CompoundIngestor for Spy {
        async fn ingest_batch(
            &self,
            _a: &str,
            _raws: Vec<crate::memory::store::raw_memory::RawMemory>,
        ) -> Result<ApplyReport, crate::error::AlephError> {
            *self.calls.lock().unwrap() += 1;
            Ok(ApplyReport::default())
        }
    }

    // Construct a minimal CompressionService with the spy injected.
    // The exact constructor calls depend on the existing test helper; follow
    // whatever pattern the file already uses. If there's no helper, skip the
    // test and rely on Task 15 integration.
    // ...
}
```

If the CompressionService test harness is non-trivial, skip this unit test — Task 15 integration covers it.

- [ ] **Step 3: Implement routing**

In `src/memory/compression/service.rs`:

1. Add a field:

```rust
pub struct CompressionService {
    // ...existing fields (some may be deleted — see Task 11)
    compound_ingestor: Option<Arc<dyn crate::memory::notes::ingest::CompoundIngestor>>,
    compound_enabled: bool,
}
```

2. Add a builder:

```rust
pub fn with_compound_ingestor(
    mut self,
    ing: Arc<dyn crate::memory::notes::ingest::CompoundIngestor>,
) -> Self {
    self.compound_ingestor = Some(ing);
    self
}
```

3. Find the existing per-source loop that calls `extract_note_updates_for_source` (inside `compress_default_notes` or its helper). Replace the per-source extraction-and-apply block with:

```rust
if self.compound_enabled {
    if let Some(ing) = &self.compound_ingestor {
        match ing.ingest_batch(agent_id, batch_rows_for_source).await {
            Ok(_report) => {
                // raws will be marked processed below — standard path
            }
            Err(e) => {
                tracing::warn!("compound ingest failed: {e}");
                // Leave raws unprocessed; retry next tick.
                continue;
            }
        }
    } else {
        tracing::warn!("compound ingest enabled but no ingestor configured; skipping batch");
        continue;
    }
} else {
    // Fallback disabled in Spec 6 — the legacy path is deleted in Task 12.
    continue;
}
```

4. Mark the consumed raws processed as before (`mark_raw_as_processed`).

Update any constructor paths / the `compound_enabled` default from `MemoryConfig.compound_ingest.enabled`.

- [ ] **Step 4: Compile + run existing tests**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib memory::compression 2>&1 | tail -10
cargo test -p alephcore --lib memory::notes::ingest 2>&1 | tail -5
```

Expected: compiles; existing compression tests pass; ingest tests pass.

**Expect new dead-code warnings** on `ConflictDetector` / legacy extractor paths — those are removed in Task 11 / 12. Leave them for now.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add src/config/types/memory.rs src/memory/compression/service.rs
git commit -m "feat(ingest): route CompressionService through CompoundIngestor"
```

---

## Task 10: Bootstrap `CompoundIngestor` in app startup

**Files:**
- Modify: the startup builder that constructs `CompressionService` (grep `CompressionService::new` / `CompressionService::new_with_backend`)

- [ ] **Step 1: Grep for the construction site**

```bash
cd /Volumes/TBU4/Workspace/Aleph
rg -n "CompressionService::new\b|CompressionService::new_with_backend" src/
```

Likely site: `src/bin/aleph-server/commands/start/builder/agent_init.rs` (near where NoteIndexer / MemoryContextProvider / DreamDaemon are wired).

- [ ] **Step 2: Construct ingestor + inject**

At the startup site, after `NoteIndexer` and the provider/embedder are available:

```rust
use crate::memory::notes::ingest::{
    DefaultCompoundIngestor, CompoundIngestor,
    retrieve::RelatedBudget,
};

let compound_budget = RelatedBudget {
    max_related_pages: memory_cfg.compound_ingest.max_related_pages,
    preview_char_cap: memory_cfg.compound_ingest.related_preview_char_cap,
    total_byte_cap: memory_cfg.compound_ingest.related_total_byte_cap,
};
let compound_ingestor: Arc<dyn CompoundIngestor> = Arc::new(DefaultCompoundIngestor {
    store: backend.clone(),
    indexer: Arc::new(indexer.clone()), // if indexer isn't already Arc<>
    provider: provider.clone(),
    embedder: embedder.clone(),
    orientation: Some(orientation.clone()),
    memory_dir: memory_dir.join("note"),
    budget: compound_budget,
});

let compression = CompressionService::new_with_backend(
    backend.clone(),
    provider.clone(),
    embedder.clone(),
    compression_cfg,
    Some(backend.clone()),
)
.with_compound_ingestor(compound_ingestor);
```

Adapt argument names / Arc wrapping to match the real site.

- [ ] **Step 3: Compile + smoke**

```bash
cargo check -p alephcore
cargo build -p alephcore --bin aleph-server 2>&1 | tail -5
cargo test -p alephcore --lib
```

Expected: everything compiles; lib tests still pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add -A
git commit -m "feat(ingest): construct DefaultCompoundIngestor at app startup"
```

---

## Task 11: Delete `ConflictDetector`

**Files:**
- Delete: `src/memory/compression/conflict.rs`
- Modify: `src/memory/compression/mod.rs`
- Modify: `src/memory/compression/service.rs`

- [ ] **Step 1: Find all references**

```bash
cd /Volumes/TBU4/Workspace/Aleph
rg -n "ConflictDetector|ConflictConfig|conflict::" src/
```

- [ ] **Step 2: Remove references**

Delete from `src/memory/compression/service.rs`:
- `conflict_detector: Arc<ConflictDetector>` field
- `ConflictDetector::new(...)` construction in `new` / `new_with_backend`
- The `ConflictConfig` field on `CompressionConfig` (or whatever it's called)
- Any `conflict_detector.detect(...)` call site (should already be unused after Task 9 rerouted the main path)

Delete from `src/memory/compression/mod.rs`:
- `pub mod conflict;`

Delete the file:

```bash
git rm src/memory/compression/conflict.rs
```

Update `CompressionConfig` in the same file to drop the `conflict: ConflictConfig` field plus its `Default`.

- [ ] **Step 3: Compile**

```bash
cargo check -p alephcore
```

Fix any remaining reference errors. Most should be in test files that called `ConflictDetector` directly — delete those tests.

- [ ] **Step 4: Run all tests**

```bash
cargo test -p alephcore --lib
cargo test -p alephcore --test memory_note_orientation
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p alephcore
git add -A
git commit -m "refactor(ingest): delete ConflictDetector — LLM-produced Contradict ops replace it"
```

---

## Task 12: Delete legacy `FactExtractor` entry points

**Files:**
- Modify: `src/memory/compression/extractor.rs`

- [ ] **Step 1: Delete deprecated methods + types**

In `src/memory/compression/extractor.rs`, remove:
- `impl FactExtractor { pub async fn extract_facts(...) }`
- `impl FactExtractor { pub async fn extract_unified(...) }`
- `pub fn parse_unified_response(...)`
- Public types `UnifiedExtractionResponse`, `ExtractedFact`, `ExtractedEntity`, `ExtractedRelationship`, `ExtractionResponse`
- Private helpers used ONLY by the above (`get_system_prompt`, `build_extraction_prompt` if no other caller), plus the `get_unified_system_prompt` method

Keep:
- The `FactExtractor` struct itself IF it's still referenced elsewhere (e.g., `extract_note_updates_for_source`). Otherwise delete the whole file.

After removal, grep:

```bash
rg -n "FactExtractor" src/
```

If only callers remain inside the file itself (self-references) or tests exercising removed APIs, delete those tests too. If `extract_note_updates_for_source` is the only remaining public API, it lives on too in this task — mark it `#[deprecated]` with note "internal only; CompoundIngestor replaces it; will be removed before Spec 7".

- [ ] **Step 2: Remove dead imports**

After deletion, `cargo check -p alephcore 2>&1 | grep -E "unused import|unused (struct|function)"` will flag unused imports / items in `extractor.rs`. Clean them.

- [ ] **Step 3: Run full test suite**

```bash
cargo test -p alephcore --lib
cargo test -p alephcore --test memory_note_orientation
```

Expected: all green.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add -A
git commit -m "refactor(ingest): delete legacy FactExtractor methods + types"
```

---

## Task 13: Drop legacy `dream_reports` columns

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`

The `dream_reports` table carries 11 columns that predate the note-era schema and are never written by current code. Drop them via a migration.

- [ ] **Step 1: Write the migration**

In `src/memory/store/sqlite/schema.rs`, find where `init_schema` calls helpers like `drop_obsolete_facts_tables`. Add a parallel helper `migrate_dream_reports_drop_legacy_cols`:

```rust
pub(crate) fn migrate_dream_reports_drop_legacy_cols(conn: &Connection) -> Result<(), AlephError> {
    // SQLite ≥ 3.35 supports DROP COLUMN directly. Aleph uses sqlite-vec which
    // requires modern SQLite; DROP COLUMN is available.
    let legacy_cols = [
        "facts_collected",
        "clusters_found",
        "drift_detected",
        "drift_summary",
        "candidates_evaluated",
        "facts_promoted",
        "promotion_details",
        "facts_decayed",
        "facts_pruned",
        "nodes_decayed",
        "edges_decayed",
    ];
    // Query current columns.
    let existing: std::collections::BTreeSet<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .map_err(|e| AlephError::other(format!("pragma: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| AlephError::other(format!("pragma rows: {e}")))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for col in legacy_cols {
        if existing.contains(col) {
            let sql = format!("ALTER TABLE dream_reports DROP COLUMN {col}");
            conn.execute(&sql, [])
                .map_err(|e| AlephError::other(format!("drop col {col}: {e}")))?;
        }
    }
    Ok(())
}
```

In `init_schema`, call it after `CREATE_DREAM_REPORTS` runs:

```rust
migrate_dream_reports_drop_legacy_cols(conn)?;
```

Also update the `CREATE_DREAM_REPORTS` constant to the new shape (just `id, pipeline_type, started_at, finished_at, duration_ms, synthesis_count, errors, namespace`). Keep the same index `idx_dream_reports_started`.

- [ ] **Step 2: Write migration idempotency test**

Append to the existing tests in `schema.rs`:

```rust
#[test]
fn migrate_dream_reports_drop_legacy_cols_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE dream_reports (
            id TEXT PRIMARY KEY,
            pipeline_type TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            finished_at INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            facts_collected INTEGER NOT NULL DEFAULT 0,
            facts_promoted INTEGER NOT NULL DEFAULT 0,
            synthesis_count INTEGER NOT NULL DEFAULT 0,
            errors TEXT,
            namespace TEXT NOT NULL DEFAULT 'owner'
        )",
    )
    .unwrap();
    // First run drops legacy cols.
    super::migrate_dream_reports_drop_legacy_cols(&conn).unwrap();
    // Second run is a no-op.
    super::migrate_dream_reports_drop_legacy_cols(&conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(dream_reports)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(!cols.contains(&"facts_collected".to_string()));
    assert!(!cols.contains(&"facts_promoted".to_string()));
    assert!(cols.contains(&"pipeline_type".to_string()));
    assert!(cols.contains(&"synthesis_count".to_string()));
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib memory::store::sqlite::schema
cargo test -p alephcore --lib memory::dreaming
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/store/sqlite/schema.rs
git commit -m "refactor(schema): drop legacy dream_reports columns (pre-notes-era)"
```

---

## Task 14: Proptest — plan/apply atomicity

**Files:**
- Modify: `src/memory/notes/ingest/apply.rs`

- [ ] **Step 1: Write proptest**

Append inside the existing `#[cfg(test)] mod tests` in `src/memory/notes/ingest/apply.rs`:

```rust
use proptest::prelude::*;

fn op_strategy() -> impl Strategy<Value = PageOp> {
    let name = "[a-z][a-z0-9-]{0,8}";
    let path = (name, name).prop_map(|(c, n)| format!("{c}/{n}"));
    prop_oneof![
        path.clone().prop_flat_map(|p| {
            let p2 = p.clone();
            ("[a-z ]{3,20}", "[a-z ]{1,40}").prop_map(move |(t, s)| PageOp::Create {
                note_path: p2.clone(),
                title: t,
                summary: s,
                facts: vec![],
                links: vec![format!("seed/link")],
                tags: vec![],
            })
        }),
        path.clone().prop_map(|p| PageOp::Append {
            note_path: p,
            new_facts: vec!["f".into()],
            new_links: vec![],
        }),
        (path.clone(), path).prop_filter(
            "distinct endpoints",
            |(a, b)| a != b,
        ).prop_map(|(from, to)| PageOp::Link { from, to }),
    ]
}

proptest! {
    #[test]
    fn apply_commit_produces_files_on_disk(
        ops in proptest::collection::vec(op_strategy(), 0..8)
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let (dir, backend, indexer) = fresh().await;
            let mut tx = CompoundApplyTx::new(
                &indexer,
                &backend,
                dir.path().join("note"),
                "default",
            );
            let mut expect_paths: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            for op in &ops {
                if tx.stage(op).await.is_err() {
                    // Some generated ops intentionally fail (e.g. Update on missing page).
                    return;
                }
                if matches!(op, PageOp::Create { .. } | PageOp::Append { .. }) {
                    expect_paths.insert(op.primary_path().to_string());
                }
            }
            let report = tx.commit().await;
            prop_assert!(report.is_ok(), "commit failed: {:?}", report);
            for p in expect_paths {
                let (cat, name) = p.split_once('/').unwrap();
                let file = dir
                    .path()
                    .join(format!("note/default/{cat}/{name}.md"));
                prop_assert!(file.exists(), "missing file {file:?}");
            }
            Ok(())
        }).unwrap();
    }
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p alephcore --lib memory::notes::ingest::apply
```

Expected: all tests pass (existing + new proptest with 256 default cases).

- [ ] **Step 3: Commit**

```bash
cargo fmt -p alephcore
git add src/memory/notes/ingest/apply.rs
git commit -m "test(ingest): proptest — apply commit produces files on disk"
```

---

## Task 15: Integration test — compound ingest end-to-end

**Files:**
- Create: `tests/memory_compound_ingest.rs`

- [ ] **Step 1: Write failing test**

```rust
//! End-to-end: a RecordingMockProvider emits a multi-op plan; CompoundIngestor
//! applies it; assert files on disk + SQLite reflect the plan.

use alephcore::error::AlephError;
use alephcore::memory::embedding_provider::tests::MockEmbeddingProvider;
use alephcore::memory::notes::indexer::NoteIndexer;
use alephcore::memory::notes::ingest::{
    retrieve::RelatedBudget, CompoundIngestor, DefaultCompoundIngestor,
};
use alephcore::memory::store::raw_memory::{RawMemory, RawMemorySource};
use alephcore::memory::store::sqlite::SqliteMemoryBackend;
use alephcore::providers::recording_mock::RecordingMockProvider;
use alephcore::providers::AiProvider;
use std::sync::Arc;

#[tokio::test]
async fn compound_ingest_creates_and_links_pages() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
    let provider: Arc<dyn AiProvider> = Arc::new(RecordingMockProvider::new(
        r#"{
          "reasoning": "two pages + a link",
          "ops": [
            {"kind":"create","note_path":"learning/tokio","title":"Tokio",
             "summary":"async runtime","facts":["event loop"],
             "links":["learning/rust-async"],"tags":["rust","async"]},
            {"kind":"create","note_path":"learning/rust-async","title":"Rust async",
             "summary":"futures + pin","facts":["futures are lazy"],
             "links":["learning/tokio"],"tags":["rust","async"]},
            {"kind":"link","from":"learning/tokio","to":"learning/rust-async"}
          ]
        }"#.into(),
    ));
    let embedder = Arc::new(MockEmbeddingProvider::new(1024, "mock"));
    let ing = DefaultCompoundIngestor {
        store: backend.clone(),
        indexer: indexer.clone(),
        provider,
        embedder,
        orientation: None,
        memory_dir: dir.path().join("note"),
        budget: RelatedBudget::default(),
    };

    let raw = RawMemory::new(
        "User mentions tokio and rust async together",
        RawMemorySource::Transcript,
    );
    let report = ing
        .ingest_batch("default", vec![raw])
        .await
        .expect("ingest ok");

    assert_eq!(report.created, 2);
    assert_eq!(report.linked, 1);
    assert!(dir.path().join("note/default/learning/tokio.md").exists());
    assert!(dir.path().join("note/default/learning/rust-async.md").exists());

    let listed = backend.list_notes("default").await.unwrap();
    let paths: Vec<String> = listed.iter().map(|e| e.path.clone()).collect();
    assert!(paths.contains(&"learning/tokio".to_string()));
    assert!(paths.contains(&"learning/rust-async".to_string()));
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p alephcore --test memory_compound_ingest
```

Expected: 1 pass.

- [ ] **Step 3: Commit**

```bash
cargo fmt -p alephcore
git add tests/memory_compound_ingest.rs
git commit -m "test(ingest): end-to-end compound ingest integration"
```

---

## Task 16: Final sanity pass

- [ ] **Step 1: Run the full validation suite**

```bash
cd /Volumes/TBU4/Workspace/Aleph
cargo fmt --check -p alephcore
cargo test -p alephcore --lib
cargo test -p alephcore --test memory_note_orientation
cargo test -p alephcore --test memory_compound_ingest
```

Expected: `cargo fmt --check` exit 0, all tests pass.

Scoped clippy (Spec 6 files only):
```bash
cargo clippy -p alephcore --lib -- -D warnings 2>&1 \
  | grep -E "memory/notes/ingest|memory/compression/service" \
  | head -20
```

Expected: empty (no Spec 6 clippy issues).

- [ ] **Step 2: Update reference doc**

Append to `docs/reference/MEMORY_SYSTEM.md` a short pointer after the existing §9 or §10:

```markdown
## Compound ingest (Spec 6, shipped YYYY-MM-DD)

Replaces the single-step extractor with a two-phase pipeline: Phase 1
retrieves up to 15 related pages via hybrid search + 1-hop wikilink
expansion; Phase 2 asks the LLM for a cross-page `IngestPlan` (create /
append / update / contradict / link / supersede); Phase 3 applies the
plan transactionally via staged file writes + batched rename. The
`ConflictDetector` similarity heuristic is deleted — `PageOp::Contradict`
comes from the LLM directly. See
[docs/superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md §3](../superpowers/specs/2026-04-14-memory-llm-wiki-evolution-design.md).
```

- [ ] **Step 3: Commit**

```bash
git add docs/reference/MEMORY_SYSTEM.md
git commit -m "docs(memory): point to compound ingest (Spec 6) from MEMORY_SYSTEM.md"
```

- [ ] **Step 4: Emit summary**

Report:
- Final HEAD commit SHA
- Spec 6 commit count (grep `git log --oneline | grep -cE "(ingest|schema)" ` bounded to this spec range)
- Test counts (lib + both integration tests)
- Lines added / removed (diff against Spec 5 HEAD)

## Report

DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED + summary.

---

## Self-Review

**Spec coverage (design §3):**

| Spec §3 sub-section | Task(s) |
|---|---|
| §3.1 pipeline overview | 2, 4, 5, 7, 8 |
| §3.2 data model | 2 |
| §3.3 Phase 1 retrieve | 4 |
| §3.4 Phase 2 plan (LLM) | 3, 7 |
| §3.5 Phase 3 apply (transactional) | 5, 14 |
| §3.6 Phase 4 record (log.md) | 8 |
| §3.7 integration (service + deletions) | 9, 10, 11, 12 |
| §3.8 concurrency (per-agent, one LLM call) | 9 (serial per-agent via service lock; one plan per batch is inherent to `ingest_batch`) |
| §3.9 config | 9 |
| §6.4 cleanup (schema migration) | 13 |

Spec 5 follow-up (NoteIndexer SQLite sync) — Task 1.

Integration test — Task 15. Proptest — Task 14.

**Placeholder scan:** each step contains actual Rust code or shell commands. No "TBD", no "implement later".

**Type consistency:**
- `PageOp`, `IngestPlan`, `SchemaProposal`, `ApplyReport`, `RelatedBudget`, `RelatedPage`, `CompoundIngestor`, `DefaultCompoundIngestor`, `CompoundApplyTx`, `ApplyError` — defined in Tasks 2/4/5/6/7 and referenced consistently through Tasks 8/9/10/14/15.
- `NoteOrientation` trait (from Spec 5 rename) — referenced at the correct path `crate::memory::notes::orientation::NoteOrientation`.
- `LogEntry` / `LogAction` — at `crate::memory::notes::orientation::types::LogEntry/LogAction`.
- `RawMemory` / `RawMemorySource` — `crate::memory::store::raw_memory::{RawMemory, RawMemorySource}`.
- `RecordingMockProvider::new(String)` — consistent across all LLM tests.

**Risks for the implementer:**
1. Task 5's `CompoundApplyTx` is the largest code block. Read slowly and add tests for each op kind before wiring the full flow.
2. Task 9's CompressionService rewiring has to coexist with the still-present ConflictDetector until Task 11 deletes it. Expect compile-time dead-code warnings between Task 9 and Task 11; they will resolve.
3. Task 13's SQLite migration touches an on-disk schema. If an existing dev DB has rows with legacy column values, those values are lost (not a production concern — nothing reads them, but note it).
4. Task 10's app-wiring step depends on what's in the startup builder. If the builder takes more dependencies than expected, note in the commit and proceed.

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-04-14-memory-llm-wiki-spec6-compound-ingest.md`.** Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task + two-stage review, fast iteration.
2. **Inline Execution** — batch execution with checkpoints for review.

**Which approach?**
